use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pdf_oxide::PdfDocument;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use zip::ZipArchive;

use crate::db;
use crate::domain::{ProviderRuntimeConfig, ProviderView};

const MINERU_POLL_ATTEMPTS: usize = 90;
const MINERU_INITIAL_POLL_DELAY_MS: u64 = 2_000;
const MINERU_MAX_POLL_DELAY_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PdfParsingMode {
    Local,
    Mineru,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsePdfDocumentInput {
    pub file_path: String,
    pub mode: PdfParsingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPdfDocument {
    pub source_path: String,
    pub mode: PdfParsingMode,
    pub markdown: String,
    pub page_count: Option<usize>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MinerUEnvelope {
    code: Value,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Value,
}

pub async fn parse_pdf_document(
    provider_pool: &SqlitePool,
    client: &Client,
    input: ParsePdfDocumentInput,
) -> Result<ParsedPdfDocument, String> {
    let source_path = PathBuf::from(input.file_path.trim());
    validate_pdf_path(&source_path)?;
    match input.mode {
        PdfParsingMode::Local => parse_local_pdf(source_path).await,
        PdfParsingMode::Mineru => parse_mineru_pdf(provider_pool, client, source_path).await,
    }
}

async fn parse_local_pdf(source_path: PathBuf) -> Result<ParsedPdfDocument, String> {
    let display_path = source_path.to_string_lossy().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let document = PdfDocument::open(&source_path)
            .map_err(|error| format!("Unable to open PDF with pdf_oxide: {error}"))?;
        let page_count = document
            .page_count()
            .map_err(|error| format!("Unable to read PDF page count: {error}"))?;
        let mut diagnostics = Vec::new();
        let mut markdown = String::new();
        for page_index in 0..page_count {
            let text = document.extract_text(page_index).map_err(|error| {
                format!(
                    "Unable to extract text from PDF page {}: {error}",
                    page_index + 1
                )
            })?;
            if page_count > 1 {
                if !markdown.is_empty() {
                    markdown.push_str("\n\n");
                }
                markdown.push_str(&format!("<!-- Page {} -->\n\n", page_index + 1));
            }
            markdown.push_str(text.trim());
        }
        if markdown.trim().is_empty() {
            diagnostics.push(
                "No extractable text was found. This PDF may need MinerU OCR parsing.".into(),
            );
        }
        Ok(ParsedPdfDocument {
            source_path: display_path,
            mode: PdfParsingMode::Local,
            markdown,
            page_count: Some(page_count),
            diagnostics,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn parse_mineru_pdf(
    provider_pool: &SqlitePool,
    client: &Client,
    source_path: PathBuf,
) -> Result<ParsedPdfDocument, String> {
    let provider = db::get_provider(provider_pool, db::MINERU_PROVIDER_ID).await?;
    let config = db::runtime_config(provider_pool, db::MINERU_PROVIDER_ID).await?;
    validate_mineru_standard_config(&provider, &config)?;

    let file_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "PDF file name is invalid".to_string())?
        .to_string();
    let model_version = provider
        .models
        .first()
        .map(|model| model.request_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vlm".into());

    let upload_plan =
        create_mineru_upload_plan(client, &provider, &config, &file_name, &model_version).await?;
    let bytes = tokio::fs::read(&source_path)
        .await
        .map_err(|error| format!("Unable to read PDF for MinerU upload: {error}"))?;
    upload_mineru_file(client, &upload_plan.file_url, bytes).await?;
    let result = wait_for_mineru_result(client, &provider, &config, &upload_plan.batch_id).await?;
    let zip_url = result
        .full_zip_url
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            result
                .error
                .unwrap_or_else(|| "MinerU finished without a result ZIP URL".into())
        })?;
    let zip_bytes = client
        .get(&zip_url)
        .send()
        .await
        .map_err(|error| format!("Unable to download MinerU result ZIP: {error}"))?
        .error_for_status()
        .map_err(|error| format!("MinerU result ZIP download failed: {error}"))?
        .bytes()
        .await
        .map_err(|error| format!("Unable to read MinerU result ZIP: {error}"))?
        .to_vec();
    let markdown = markdown_from_zip(&zip_bytes)?;

    Ok(ParsedPdfDocument {
        source_path: source_path.to_string_lossy().to_string(),
        mode: PdfParsingMode::Mineru,
        markdown,
        page_count: result.total_pages,
        diagnostics: Vec::new(),
    })
}

struct MinerUUploadPlan {
    batch_id: String,
    file_url: String,
}

#[derive(Default)]
struct MinerUTaskResult {
    state: String,
    full_zip_url: Option<String>,
    error: Option<String>,
    total_pages: Option<usize>,
}

async fn create_mineru_upload_plan(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    file_name: &str,
    model_version: &str,
) -> Result<MinerUUploadPlan, String> {
    let response = client
        .post(mineru_endpoint(&provider.base_url, "file-urls/batch"))
        .headers(mineru_headers(config)?)
        .json(&json!({
            "files": [{ "name": file_name }],
            "model_version": model_version,
        }))
        .send()
        .await
        .map_err(|error| format!("Unable to request MinerU upload URL: {error}"))?;
    let envelope = parse_mineru_response(response).await?;
    let batch_id = envelope
        .data
        .get("batch_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "MinerU upload response did not include batch_id".to_string())?
        .to_string();
    let file_url = envelope
        .data
        .get("file_urls")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .ok_or_else(|| "MinerU upload response did not include file_urls[0]".to_string())?
        .to_string();
    Ok(MinerUUploadPlan { batch_id, file_url })
}

async fn upload_mineru_file(client: &Client, file_url: &str, bytes: Vec<u8>) -> Result<(), String> {
    client
        .put(file_url)
        .body(bytes)
        .send()
        .await
        .map_err(|error| format!("Unable to upload PDF to MinerU file URL: {error}"))?
        .error_for_status()
        .map_err(|error| format!("MinerU file upload failed: {error}"))?;
    Ok(())
}

async fn wait_for_mineru_result(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    batch_id: &str,
) -> Result<MinerUTaskResult, String> {
    let mut delay = Duration::from_millis(MINERU_INITIAL_POLL_DELAY_MS);
    for _ in 0..MINERU_POLL_ATTEMPTS {
        let response = client
            .get(mineru_endpoint(
                &provider.base_url,
                &format!("extract-results/batch/{batch_id}"),
            ))
            .headers(mineru_headers(config)?)
            .send()
            .await
            .map_err(|error| format!("Unable to poll MinerU result: {error}"))?;
        let envelope = parse_mineru_response(response).await?;
        let result = first_mineru_task_result(&envelope.data)?;
        match result.state.as_str() {
            "done" => return Ok(result),
            "failed" => {
                return Err(result
                    .error
                    .unwrap_or_else(|| "MinerU PDF parsing failed".into()))
            }
            _ => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(MINERU_MAX_POLL_DELAY_MS));
            }
        }
    }
    Err("MinerU PDF parsing timed out".into())
}

async fn parse_mineru_response(response: reqwest::Response) -> Result<MinerUEnvelope, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Unable to read MinerU response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "MinerU returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(500).collect::<String>()
        ));
    }
    let envelope: MinerUEnvelope = serde_json::from_str(&body)
        .map_err(|error| format!("Invalid MinerU response JSON: {error}"))?;
    if mineru_code_to_string(&envelope.code) != "0" {
        return Err(format!(
            "MinerU returned {}: {}",
            mineru_code_to_string(&envelope.code),
            envelope.msg.unwrap_or_else(|| "unknown error".into())
        ));
    }
    Ok(envelope)
}

fn first_mineru_task_result(data: &Value) -> Result<MinerUTaskResult, String> {
    let item = data
        .get("extract_result")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| "MinerU batch response did not include extract_result[0]".to_string())?;
    let progress = item.get("extract_progress");
    Ok(MinerUTaskResult {
        state: item
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        full_zip_url: item
            .get("full_zip_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        error: item
            .get("err_msg")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        total_pages: progress
            .and_then(|value| value.get("total_pages"))
            .and_then(Value::as_u64)
            .map(|value| value as usize),
    })
}

fn markdown_from_zip(bytes: &[u8]) -> Result<String, String> {
    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader)
        .map_err(|error| format!("Unable to open MinerU result ZIP: {error}"))?;
    let mut fallback = None;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("Unable to read MinerU ZIP entry: {error}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        if !name.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| format!("Unable to decode MinerU Markdown from ZIP: {error}"))?;
        if name.ends_with("full.md") {
            return Ok(text);
        }
        if fallback.is_none() {
            fallback = Some(text);
        }
    }
    fallback.ok_or_else(|| "MinerU result ZIP did not contain a Markdown file".into())
}

fn validate_pdf_path(path: &Path) -> Result<(), String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("pdf"))
        != Some(true)
    {
        return Err("Only .pdf files can be parsed by the PDF parser".into());
    }
    if !path.exists() {
        return Err("PDF file does not exist".into());
    }
    Ok(())
}

fn validate_mineru_standard_config(
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
) -> Result<(), String> {
    if !provider.enabled {
        return Err("MinerU document parsing provider is not enabled".into());
    }
    if db::mineru_mode(&config.config) != "standard" {
        return Err("MinerU PDF parsing requires Standard v4 mode".into());
    }
    if config
        .credential
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err("MinerU Standard mode requires an API Key".into());
    }
    Ok(())
}

fn mineru_headers(config: &ProviderRuntimeConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if let Some(credential) = config
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {credential}"))
                .map_err(|error| format!("Invalid MinerU API Key: {error}"))?,
        );
    }
    for (name, value) in &config.custom_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid custom header {name}: {error}"))?;
        headers.insert(
            header_name,
            HeaderValue::from_str(value)
                .map_err(|error| format!("Invalid custom header value for {name}: {error}"))?,
        );
    }
    Ok(headers)
}

fn mineru_endpoint(base_url: &str, suffix: &str) -> String {
    let base = base_url
        .split('#')
        .next()
        .unwrap_or(base_url)
        .trim()
        .trim_end_matches('/');
    format!("{base}/{}", suffix.trim_start_matches('/'))
}

fn mineru_code_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProviderRuntimeConfig, ProviderView, UpdateProviderConfigInput};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use zip::write::SimpleFileOptions;

    fn temp_pdf_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "insitu-pdf-parsing-{label}-{}.pdf",
            db::new_id("test")
        ))
    }

    fn provider(enabled: bool, mineru_mode: &str) -> ProviderView {
        ProviderView {
            id: db::MINERU_PROVIDER_ID.into(),
            name: "MinerU".into(),
            protocol: crate::domain::ProviderProtocol::OpenaiChat,
            base_url: db::MINERU_STANDARD_BASE_URL.into(),
            use_raw_base_url: true,
            config: json!({ "mineru": { "mode": mineru_mode } }),
            avatar: Some("mineru".into()),
            is_builtin: true,
            enabled,
            credential_mask: None,
            custom_header_keys: Vec::new(),
            purpose: crate::domain::ProviderPurpose::DocumentParsing,
            models: Vec::new(),
        }
    }

    fn runtime_config(credential: Option<&str>, mineru_mode: &str) -> ProviderRuntimeConfig {
        ProviderRuntimeConfig {
            protocol: crate::domain::ProviderProtocol::OpenaiChat,
            base_url: db::MINERU_STANDARD_BASE_URL.into(),
            use_raw_base_url: true,
            config: json!({ "mineru": { "mode": mineru_mode } }),
            auth_type: "bearer".into(),
            auth_header: "Authorization".into(),
            credential: credential.map(str::to_string),
            custom_headers: Vec::new(),
        }
    }

    fn test_zip(markdown: &str) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file("result/full.md", SimpleFileOptions::default())
                .expect("start full md");
            writer
                .write_all(markdown.as_bytes())
                .expect("write full md");
            writer.finish().expect("finish zip");
        }
        bytes.into_inner()
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&request);
                let content_length = header_text
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let body_read = request.len().saturating_sub(header_end + 4);
                    if body_read >= content_length {
                        break;
                    }
                }
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write json response");
    }

    fn write_binary_response(stream: &mut std::net::TcpStream, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("write response headers");
        stream.write_all(body).expect("write response body");
    }

    #[test]
    fn validates_pdf_path_before_parsing() {
        let path =
            std::env::temp_dir().join(format!("insitu-pdf-parsing-{}.txt", db::new_id("test")));
        std::fs::write(&path, "not a pdf").expect("write test file");
        let error = validate_pdf_path(&path).expect_err("txt file should be rejected");
        assert!(error.contains("Only .pdf files"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mineru_standard_config_requires_enabled_standard_and_key() {
        let config_with_key = runtime_config(Some("token"), "standard");
        assert!(
            validate_mineru_standard_config(&provider(false, "standard"), &config_with_key)
                .expect_err("disabled provider should fail")
                .contains("not enabled")
        );

        let enabled_provider = provider(true, "standard");
        let missing_key = runtime_config(None, "standard");
        assert!(
            validate_mineru_standard_config(&enabled_provider, &missing_key)
                .expect_err("missing API key should fail")
                .contains("API Key")
        );

        let flash_config = runtime_config(Some("token"), "flash");
        assert!(
            validate_mineru_standard_config(&enabled_provider, &flash_config)
                .expect_err("flash mode should fail")
                .contains("Standard v4")
        );

        assert!(validate_mineru_standard_config(&enabled_provider, &config_with_key).is_ok());
    }

    #[test]
    fn extracts_preferred_full_markdown_from_mineru_zip() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer
                .start_file("pages/page_1.md", options)
                .expect("start fallback md");
            writer.write_all(b"fallback").expect("write fallback");
            writer
                .start_file("result/full.md", options)
                .expect("start full md");
            writer.write_all(b"# full").expect("write full");
            writer.finish().expect("finish zip");
        }

        assert_eq!(
            markdown_from_zip(bytes.get_ref()).expect("markdown"),
            "# full"
        );
    }

    #[test]
    fn rejects_mineru_zip_without_markdown() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file("result/data.json", SimpleFileOptions::default())
                .expect("start json");
            writer.write_all(b"{}").expect("write json");
            writer.finish().expect("finish zip");
        }

        assert!(markdown_from_zip(bytes.get_ref())
            .expect_err("zip without markdown should fail")
            .contains("Markdown file"));
    }

    #[tokio::test]
    async fn mineru_mode_rejects_enabled_provider_without_api_key_before_network() {
        let path = temp_pdf_path("missing-key");
        std::fs::write(&path, b"%PDF-1.4\n% test").expect("write pdf stub");
        let db_path =
            std::env::temp_dir().join(format!("insitu-pdf-parsing-{}.sqlite3", db::new_id("test")));
        let pool = db::connect(&db_path).await.expect("connect db");
        db::set_provider_enabled(
            &pool,
            crate::domain::SetProviderEnabledInput {
                id: db::MINERU_PROVIDER_ID.into(),
                enabled: true,
            },
        )
        .await
        .expect("enable mineru");

        let error = parse_pdf_document(
            &pool,
            &Client::new(),
            ParsePdfDocumentInput {
                file_path: path.to_string_lossy().to_string(),
                mode: PdfParsingMode::Mineru,
            },
        )
        .await
        .expect_err("missing API key should fail");
        assert!(error.contains("API Key"));

        pool.close().await;
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn mineru_standard_flow_uploads_polls_downloads_zip_and_reads_markdown() {
        let pdf_path = temp_pdf_path("mock-flow");
        std::fs::write(&pdf_path, b"%PDF-1.4\n% test").expect("write pdf stub");

        let zip_bytes = test_zip("# parsed");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock mineru");
        let address = listener.local_addr().expect("mock address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept upload plan");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("POST /file-urls/batch "));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token"));
            assert!(request.contains("\"model_version\":\"vlm\""));
            write_json_response(
                &mut stream,
                &format!(
                    r#"{{"code":0,"data":{{"batch_id":"batch-1","file_urls":["http://{address}/upload"]}}}}"#
                ),
            );

            let (mut stream, _) = listener.accept().expect("accept upload");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("PUT /upload "));
            assert!(request.contains("%PDF-1.4"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("write upload response");

            let (mut stream, _) = listener.accept().expect("accept poll");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /extract-results/batch/batch-1 "));
            write_json_response(
                &mut stream,
                &format!(
                    r#"{{"code":0,"data":{{"extract_result":[{{"state":"done","full_zip_url":"http://{address}/result.zip","extract_progress":{{"total_pages":7}}}}]}}}}"#
                ),
            );

            let (mut stream, _) = listener.accept().expect("accept zip download");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /result.zip "));
            write_binary_response(&mut stream, &zip_bytes);
        });

        let db_path =
            std::env::temp_dir().join(format!("insitu-pdf-parsing-{}.sqlite3", db::new_id("test")));
        let pool = db::connect(&db_path).await.expect("connect db");
        db::update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: db::MINERU_PROVIDER_ID.into(),
                base_url: format!("http://{address}#raw"),
                use_raw_base_url: true,
                config: Some(json!({
                    "mineru": {
                        "mode": "standard",
                        "flashBaseUrl": db::MINERU_FLASH_BASE_URL,
                    },
                })),
            },
        )
        .await
        .expect("set mock base url");
        db::set_provider_enabled(
            &pool,
            crate::domain::SetProviderEnabledInput {
                id: db::MINERU_PROVIDER_ID.into(),
                enabled: true,
            },
        )
        .await
        .expect("enable mineru");
        db::replace_credential(
            &pool,
            db::MINERU_PROVIDER_ID,
            Some("test-token".to_string()),
        )
        .await
        .expect("set credential");

        let parsed = parse_pdf_document(
            &pool,
            &Client::new(),
            ParsePdfDocumentInput {
                file_path: pdf_path.to_string_lossy().to_string(),
                mode: PdfParsingMode::Mineru,
            },
        )
        .await
        .expect("parse through mock mineru");

        assert_eq!(parsed.markdown, "# parsed");
        assert_eq!(parsed.page_count, Some(7));
        assert_eq!(parsed.mode, PdfParsingMode::Mineru);

        pool.close().await;
        server.join().expect("mock server");
        let _ = std::fs::remove_file(pdf_path);
        let _ = std::fs::remove_file(db_path);
    }
}
