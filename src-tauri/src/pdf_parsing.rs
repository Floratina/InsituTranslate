use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path};
use std::time::Duration;

use pdf_oxide::converters::ConversionOptions;
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
const PDF_ASSET_ROOT: &str = "assets";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PdfParsingMode {
    LocalFirst,
    MineruFirst,
    LocalOnly,
    MineruOnly,
}

impl Default for PdfParsingMode {
    fn default() -> Self {
        Self::LocalFirst
    }
}

impl PdfParsingMode {
    fn attempts(self) -> &'static [PdfParsingAttempt] {
        match self {
            Self::LocalFirst => &[PdfParsingAttempt::Local, PdfParsingAttempt::Mineru],
            Self::MineruFirst => &[PdfParsingAttempt::Mineru, PdfParsingAttempt::Local],
            Self::LocalOnly => &[PdfParsingAttempt::Local],
            Self::MineruOnly => &[PdfParsingAttempt::Mineru],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PdfParsingEngine {
    Local,
    MineruStandard,
    MineruFlash,
}

impl PdfParsingEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::MineruStandard => "mineru-standard",
            Self::MineruFlash => "mineru-flash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfParsingAttempt {
    Local,
    Mineru,
}

impl PdfParsingAttempt {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "Local pdf_oxide",
            Self::Mineru => "MinerU",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PdfAsset {
    pub relative_path: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub source: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ParsedPdfDocument {
    pub source_path: String,
    pub requested_mode: PdfParsingMode,
    pub engine: PdfParsingEngine,
    pub markdown: String,
    pub page_count: Option<usize>,
    pub diagnostics: Vec<String>,
    pub assets: Vec<PdfAsset>,
}

#[derive(Debug, Deserialize)]
struct MinerUEnvelope {
    code: Value,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Value,
}

pub async fn parse_pdf_for_task(
    provider_pool: &SqlitePool,
    client: &Client,
    task_id: &str,
    source_path: &Path,
    mode: PdfParsingMode,
) -> Result<ParsedPdfDocument, String> {
    validate_pdf_path(source_path)?;
    let mut errors = Vec::new();
    for attempt in mode.attempts() {
        let parsed = match attempt {
            PdfParsingAttempt::Local => parse_local_pdf(task_id, source_path).await,
            PdfParsingAttempt::Mineru => {
                parse_mineru_pdf(provider_pool, client, task_id, source_path).await
            }
        };
        match parsed {
            Ok(mut parsed) => {
                parsed.requested_mode = mode;
                parsed
                    .diagnostics
                    .extend(errors.into_iter().map(|error| format!("Fallback: {error}")));
                return Ok(parsed);
            }
            Err(error) => errors.push(format!("{} failed: {error}", attempt.label())),
        }
    }
    Err(format!("PDF parsing failed. {}", errors.join("; ")))
}

async fn parse_local_pdf(task_id: &str, source_path: &Path) -> Result<ParsedPdfDocument, String> {
    let source_path = source_path.to_path_buf();
    let display_path = source_path.to_string_lossy().to_string();
    let asset_base = task_asset_root(task_id);
    tauri::async_runtime::spawn_blocking(move || {
        let document = PdfDocument::open(&source_path)
            .map_err(|error| format!("Unable to open PDF with pdf_oxide: {error}"))?;
        let page_count = document
            .page_count()
            .map_err(|error| format!("Unable to read PDF page count: {error}"))?;
        let temp_dir =
            std::env::temp_dir().join(format!("insitu-pdf-assets-{}", db::new_id("pdf")));
        std::fs::create_dir_all(&temp_dir)
            .map_err(|error| format!("Unable to create temporary PDF asset directory: {error}"))?;
        let output_dir = normalize_path_separators(&temp_dir);
        let options = ConversionOptions {
            extract_tables: true,
            include_images: true,
            image_output_dir: Some(output_dir.clone()),
            embed_images: false,
            render_formulas: false,
            ..ConversionOptions::default()
        };
        let markdown = document.to_markdown_all(&options).map_err(|error| {
            format!("Unable to convert PDF to Markdown with pdf_oxide: {error}")
        })?;
        let (markdown, assets) =
            collect_and_rewrite_local_assets(&markdown, &temp_dir, &output_dir, &asset_base)?;
        let _ = std::fs::remove_dir_all(&temp_dir);
        if !markdown_has_extractable_text(&markdown) {
            return Err(
                "No extractable text was found. This PDF may need MinerU OCR parsing.".into(),
            );
        }
        Ok(ParsedPdfDocument {
            source_path: display_path,
            requested_mode: PdfParsingMode::LocalFirst,
            engine: PdfParsingEngine::Local,
            markdown,
            page_count: Some(page_count),
            diagnostics: Vec::new(),
            assets,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn parse_mineru_pdf(
    provider_pool: &SqlitePool,
    client: &Client,
    task_id: &str,
    source_path: &Path,
) -> Result<ParsedPdfDocument, String> {
    let provider = db::get_provider(provider_pool, db::MINERU_PROVIDER_ID).await?;
    let config = db::runtime_config(provider_pool, db::MINERU_PROVIDER_ID).await?;
    validate_mineru_config(&provider, &config)?;
    match db::mineru_mode(&config.config) {
        "flash" => parse_mineru_flash_pdf(client, &provider, &config, task_id, source_path).await,
        _ => parse_mineru_standard_pdf(client, &provider, &config, task_id, source_path).await,
    }
}

async fn parse_mineru_standard_pdf(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    task_id: &str,
    source_path: &Path,
) -> Result<ParsedPdfDocument, String> {
    validate_mineru_standard_config(provider, config)?;
    let file_name = pdf_file_name(source_path)?;
    let model_version = mineru_model_version(provider);
    let upload_plan =
        create_mineru_upload_plan(client, provider, config, &file_name, &model_version).await?;
    let bytes = tokio::fs::read(source_path)
        .await
        .map_err(|error| format!("Unable to read PDF for MinerU upload: {error}"))?;
    upload_mineru_file(client, &upload_plan.file_url, bytes).await?;
    let result =
        wait_for_mineru_standard_result(client, provider, config, &upload_plan.batch_id).await?;
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
    let document = mineru_document_from_zip(&zip_bytes, task_id)?;
    if !markdown_has_extractable_text(&document.markdown) {
        return Err("MinerU returned Markdown without extractable text".into());
    }
    Ok(ParsedPdfDocument {
        source_path: source_path.to_string_lossy().to_string(),
        requested_mode: PdfParsingMode::MineruOnly,
        engine: PdfParsingEngine::MineruStandard,
        markdown: document.markdown,
        page_count: result.total_pages,
        diagnostics: Vec::new(),
        assets: document.assets,
    })
}

async fn parse_mineru_flash_pdf(
    client: &Client,
    _provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    _task_id: &str,
    source_path: &Path,
) -> Result<ParsedPdfDocument, String> {
    let file_name = pdf_file_name(source_path)?;
    let flash_base_url = db::mineru_flash_base_url(&config.config);
    let upload_plan =
        create_mineru_flash_upload_plan(client, config, &flash_base_url, &file_name).await?;
    let bytes = tokio::fs::read(source_path)
        .await
        .map_err(|error| format!("Unable to read PDF for MinerU upload: {error}"))?;
    upload_mineru_file(client, &upload_plan.file_url, bytes).await?;
    let result =
        wait_for_mineru_flash_result(client, config, &flash_base_url, &upload_plan.task_id).await?;
    let markdown_url = result
        .markdown_url
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            result
                .error
                .unwrap_or_else(|| "MinerU Flash finished without a Markdown URL".into())
        })?;
    let markdown = client
        .get(&markdown_url)
        .send()
        .await
        .map_err(|error| format!("Unable to download MinerU Flash Markdown: {error}"))?
        .error_for_status()
        .map_err(|error| format!("MinerU Flash Markdown download failed: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Unable to read MinerU Flash Markdown: {error}"))?;
    if !markdown_has_extractable_text(&markdown) {
        return Err("MinerU Flash returned Markdown without extractable text".into());
    }
    Ok(ParsedPdfDocument {
        source_path: source_path.to_string_lossy().to_string(),
        requested_mode: PdfParsingMode::MineruOnly,
        engine: PdfParsingEngine::MineruFlash,
        markdown,
        page_count: result.total_pages,
        diagnostics: Vec::new(),
        assets: Vec::new(),
    })
}

struct MinerUUploadPlan {
    batch_id: String,
    file_url: String,
}

struct MinerUFlashUploadPlan {
    task_id: String,
    file_url: String,
}

#[derive(Default)]
struct MinerUTaskResult {
    state: String,
    full_zip_url: Option<String>,
    markdown_url: Option<String>,
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
        .headers(mineru_headers(config, true)?)
        .json(&json!({
            "files": [{
                "name": file_name,
                "is_ocr": true,
            }],
            "model_version": model_version,
            "enable_table": true,
            "enable_formula": true,
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

async fn create_mineru_flash_upload_plan(
    client: &Client,
    config: &ProviderRuntimeConfig,
    base_url: &str,
    file_name: &str,
) -> Result<MinerUFlashUploadPlan, String> {
    let response = client
        .post(mineru_endpoint(base_url, "parse/file"))
        .headers(mineru_headers(config, false)?)
        .json(&json!({
            "file_name": file_name,
            "language": "ch",
            "is_ocr": true,
            "enable_table": true,
            "enable_formula": true,
        }))
        .send()
        .await
        .map_err(|error| format!("Unable to request MinerU Flash upload URL: {error}"))?;
    let envelope = parse_mineru_response(response).await?;
    let task_id = envelope
        .data
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "MinerU Flash upload response did not include task_id".to_string())?
        .to_string();
    let file_url = envelope
        .data
        .get("file_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "MinerU Flash upload response did not include a file URL".to_string())?
        .to_string();
    Ok(MinerUFlashUploadPlan { task_id, file_url })
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

async fn wait_for_mineru_standard_result(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    batch_id: &str,
) -> Result<MinerUTaskResult, String> {
    poll_mineru_result(
        || {
            mineru_endpoint(
                &provider.base_url,
                &format!("extract-results/batch/{batch_id}"),
            )
        },
        || mineru_headers(config, true),
        client,
        first_mineru_standard_task_result,
    )
    .await
}

async fn wait_for_mineru_flash_result(
    client: &Client,
    config: &ProviderRuntimeConfig,
    base_url: &str,
    task_id: &str,
) -> Result<MinerUTaskResult, String> {
    poll_mineru_result(
        || mineru_endpoint(base_url, &format!("parse/{task_id}")),
        || mineru_headers(config, false),
        client,
        mineru_flash_task_result,
    )
    .await
}

async fn poll_mineru_result<F, H, P>(
    url: F,
    headers: H,
    client: &Client,
    parse: P,
) -> Result<MinerUTaskResult, String>
where
    F: Fn() -> String,
    H: Fn() -> Result<HeaderMap, String>,
    P: Fn(&Value) -> Result<MinerUTaskResult, String>,
{
    let mut delay = Duration::from_millis(MINERU_INITIAL_POLL_DELAY_MS);
    for _ in 0..MINERU_POLL_ATTEMPTS {
        let response = client
            .get(url())
            .headers(headers()?)
            .send()
            .await
            .map_err(|error| format!("Unable to poll MinerU result: {error}"))?;
        let envelope = parse_mineru_response(response).await?;
        let result = parse(&envelope.data)?;
        match result.state.as_str() {
            "done" | "completed" | "success" => return Ok(result),
            "failed" | "error" => {
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

fn first_mineru_standard_task_result(data: &Value) -> Result<MinerUTaskResult, String> {
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
        markdown_url: item
            .get("markdown_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        error: item
            .get("err_msg")
            .and_then(Value::as_str)
            .or_else(|| item.get("error").and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        total_pages: progress
            .and_then(|value| value.get("total_pages"))
            .and_then(Value::as_u64)
            .map(|value| value as usize),
    })
}

fn mineru_flash_task_result(data: &Value) -> Result<MinerUTaskResult, String> {
    let item = data.get("result").unwrap_or(data);
    let progress = item
        .get("extract_progress")
        .or_else(|| item.get("progress"))
        .or_else(|| data.get("extract_progress"));
    Ok(MinerUTaskResult {
        state: item
            .get("state")
            .or_else(|| item.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        full_zip_url: item
            .get("full_zip_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        markdown_url: item
            .get("markdown_url")
            .or_else(|| item.get("md_url"))
            .or_else(|| data.get("markdown_url"))
            .and_then(Value::as_str)
            .map(str::to_string),
        error: item
            .get("err_msg")
            .and_then(Value::as_str)
            .or_else(|| item.get("error").and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        total_pages: progress
            .and_then(|value| value.get("total_pages"))
            .and_then(Value::as_u64)
            .map(|value| value as usize),
    })
}

struct MinerUDocument {
    markdown: String,
    assets: Vec<PdfAsset>,
}

fn mineru_document_from_zip(bytes: &[u8], task_id: &str) -> Result<MinerUDocument, String> {
    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader)
        .map_err(|error| format!("Unable to open MinerU result ZIP: {error}"))?;
    let mut markdown_candidates = Vec::<(String, String)>::new();
    let mut raw_assets = Vec::<(String, String, Vec<u8>)>::new();

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("Unable to read MinerU ZIP entry: {error}"))?;
        if file.is_dir() {
            continue;
        }
        let Some(name) = normalize_archive_entry_name(file.name()) else {
            continue;
        };
        if name.to_ascii_lowercase().ends_with(".md") {
            let mut text = String::new();
            file.read_to_string(&mut text)
                .map_err(|error| format!("Unable to decode MinerU Markdown from ZIP: {error}"))?;
            markdown_candidates.push((name, text));
            continue;
        }
        if let Some(media_type) = media_type_for_path(&name) {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| format!("Unable to read MinerU ZIP asset: {error}"))?;
            raw_assets.push((name, media_type.to_string(), bytes));
        }
    }

    let (markdown_name, markdown) = select_mineru_markdown(markdown_candidates)?;
    let markdown_parent = path_parent(&markdown_name);
    let asset_base = task_asset_root(task_id);
    let mut used_paths = HashSet::new();
    let mut mapping = HashMap::new();
    let mut assets = Vec::new();

    for (entry_name, media_type, bytes) in raw_assets {
        let display_path = asset_display_path(&entry_name, markdown_parent.as_deref());
        let asset_path = unique_asset_path(&asset_base, &display_path, &mut used_paths)?;
        mapping.insert(
            normalize_markdown_reference(&display_path),
            asset_path.clone(),
        );
        mapping.insert(
            normalize_markdown_reference(&format!("./{display_path}")),
            asset_path.clone(),
        );
        mapping.insert(
            normalize_markdown_reference(&entry_name),
            asset_path.clone(),
        );
        assets.push(PdfAsset {
            relative_path: asset_path,
            media_type,
            bytes,
            source: PdfParsingEngine::MineruStandard.as_str().into(),
        });
    }

    Ok(MinerUDocument {
        markdown: rewrite_markdown_image_links(&markdown, &mapping),
        assets,
    })
}

fn select_mineru_markdown(candidates: Vec<(String, String)>) -> Result<(String, String), String> {
    let mut fallback = None;
    for (name, text) in candidates {
        if name.ends_with("full.md") {
            return Ok((name, text));
        }
        if fallback.is_none() {
            fallback = Some((name, text));
        }
    }
    fallback.ok_or_else(|| "MinerU result ZIP did not contain a Markdown file".into())
}

fn collect_and_rewrite_local_assets(
    markdown: &str,
    temp_dir: &Path,
    output_dir: &str,
    asset_base: &str,
) -> Result<(String, Vec<PdfAsset>), String> {
    let mut used_paths = HashSet::new();
    let mut mapping = HashMap::new();
    let mut assets = Vec::new();
    collect_local_assets_recursive(
        temp_dir,
        temp_dir,
        output_dir,
        asset_base,
        &mut used_paths,
        &mut mapping,
        &mut assets,
    )?;
    Ok((rewrite_markdown_image_links(markdown, &mapping), assets))
}

fn collect_local_assets_recursive(
    root: &Path,
    current: &Path,
    output_dir: &str,
    asset_base: &str,
    used_paths: &mut HashSet<String>,
    mapping: &mut HashMap<String, String>,
    assets: &mut Vec<PdfAsset>,
) -> Result<(), String> {
    if !current.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)
        .map_err(|error| format!("Unable to read temporary PDF asset directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Unable to read PDF asset entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_local_assets_recursive(
                root, &path, output_dir, asset_base, used_paths, mapping, assets,
            )?;
            continue;
        }
        let Some(media_type) = media_type_for_path(&path.to_string_lossy()) else {
            continue;
        };
        let local_relative = path
            .strip_prefix(root)
            .map_err(|error| format!("Unable to resolve PDF asset path: {error}"))?;
        let local_relative = normalize_path_separators(local_relative);
        let asset_path = unique_asset_path(asset_base, &local_relative, used_paths)?;
        let markdown_path = format!("{}/{}", output_dir.trim_end_matches('/'), local_relative);
        mapping.insert(
            normalize_markdown_reference(&markdown_path),
            asset_path.clone(),
        );
        mapping.insert(
            normalize_markdown_reference(&local_relative),
            asset_path.clone(),
        );
        assets.push(PdfAsset {
            relative_path: asset_path,
            media_type: media_type.into(),
            bytes: std::fs::read(&path)
                .map_err(|error| format!("Unable to read PDF asset bytes: {error}"))?,
            source: PdfParsingEngine::Local.as_str().into(),
        });
    }
    Ok(())
}

fn unique_asset_path(
    asset_base: &str,
    relative_path: &str,
    used_paths: &mut HashSet<String>,
) -> Result<String, String> {
    let relative_path = sanitize_asset_relative_path(relative_path)?;
    let mut candidate = format!("{}/{}", asset_base.trim_end_matches('/'), relative_path);
    let mut counter = 2;
    while !used_paths.insert(candidate.clone()) {
        let (stem, extension) = split_extension(&relative_path);
        candidate = format!(
            "{}/{}-{}{}",
            asset_base.trim_end_matches('/'),
            stem,
            counter,
            extension
        );
        counter += 1;
    }
    Ok(candidate)
}

fn sanitize_asset_relative_path(value: &str) -> Result<String, String> {
    let value = value.replace('\\', "/").trim_start_matches('/').to_string();
    let path = Path::new(&value);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| "PDF asset path contains invalid UTF-8".to_string())?;
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(
                    "PDF asset path must be relative and cannot contain parent segments".into(),
                )
            }
        }
    }
    if parts.is_empty() {
        return Err("PDF asset path cannot be empty".into());
    }
    Ok(parts.join("/"))
}

fn split_extension(value: &str) -> (&str, &str) {
    let Some(index) = value.rfind('.') else {
        return (value, "");
    };
    if value[index..].contains('/') {
        (value, "")
    } else {
        (&value[..index], &value[index..])
    }
}

fn rewrite_markdown_image_links(markdown: &str, mapping: &HashMap<String, String>) -> String {
    if mapping.is_empty() || !markdown.contains("![") {
        return markdown.to_string();
    }
    let mut output = String::with_capacity(markdown.len());
    let mut cursor = 0_usize;
    while let Some(offset) = markdown[cursor..].find("![") {
        let start = cursor + offset;
        let Some(label_end_relative) = markdown[start + 2..].find("](") else {
            break;
        };
        let target_start = start + 2 + label_end_relative + 2;
        let Some(target_end_relative) = markdown[target_start..].find(')') else {
            break;
        };
        let target_end = target_start + target_end_relative;
        output.push_str(&markdown[cursor..target_start]);
        let target = &markdown[target_start..target_end];
        output.push_str(&rewrite_markdown_image_target(target, mapping));
        cursor = target_end;
    }
    output.push_str(&markdown[cursor..]);
    output
}

fn rewrite_markdown_image_target(target: &str, mapping: &HashMap<String, String>) -> String {
    let trimmed_start = target.len() - target.trim_start().len();
    let prefix = &target[..trimmed_start];
    let rest = &target[trimmed_start..];
    let (path, suffix) = markdown_target_path_and_suffix(rest);
    let normalized = normalize_markdown_reference(path);
    match mapping.get(&normalized) {
        Some(mapped) => format!("{prefix}{mapped}{suffix}"),
        None => target.to_string(),
    }
}

fn markdown_target_path_and_suffix(target: &str) -> (&str, &str) {
    if let Some(stripped) = target.strip_prefix('<') {
        if let Some(end) = stripped.find('>') {
            return (&stripped[..end], &stripped[end + 1..]);
        }
    }
    for (index, character) in target.char_indices() {
        if character.is_ascii_whitespace() {
            return (&target[..index], &target[index..]);
        }
    }
    (target, "")
}

fn normalize_markdown_reference(value: &str) -> String {
    value
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn normalize_archive_entry_name(value: &str) -> Option<String> {
    let value = value.replace('\\', "/");
    if value.starts_with('/') || value.contains("../") || value == ".." {
        return None;
    }
    Some(value.trim_start_matches("./").to_string())
}

fn asset_display_path(entry_name: &str, markdown_parent: Option<&str>) -> String {
    markdown_parent
        .and_then(|parent| entry_name.strip_prefix(&format!("{parent}/")))
        .unwrap_or(entry_name)
        .to_string()
}

fn path_parent(value: &str) -> Option<String> {
    value
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
}

fn markdown_has_extractable_text(markdown: &str) -> bool {
    markdown.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed == "---"
            || trimmed.starts_with("<!--")
            || trimmed.starts_with("![")
            || trimmed.starts_with("> [OCR REQUIRED")
            || trimmed.starts_with("> This page is a scanned")
        {
            return false;
        }
        trimmed.chars().any(|character| character.is_alphanumeric())
    })
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

fn validate_mineru_config(
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
) -> Result<(), String> {
    if !provider.enabled {
        return Err("MinerU document parsing provider is not enabled".into());
    }
    if db::mineru_mode(&config.config) == "standard" {
        validate_mineru_standard_config(provider, config)?;
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

fn mineru_headers(
    config: &ProviderRuntimeConfig,
    include_authorization: bool,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if include_authorization {
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

fn pdf_file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "PDF file name is invalid".to_string())
        .map(str::to_string)
}

fn mineru_model_version(provider: &ProviderView) -> String {
    provider
        .models
        .first()
        .map(|model| model.request_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vlm".into())
}

fn task_asset_root(task_id: &str) -> String {
    format!("{PDF_ASSET_ROOT}/{}", sanitize_asset_segment(task_id))
}

fn sanitize_asset_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect::<String>();
    if sanitized.is_empty() {
        "pdf".into()
    } else {
        sanitized
    }
}

fn normalize_path_separators(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().replace('\\', "/")
}

fn media_type_for_path(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProviderRuntimeConfig, ProviderView};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

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
            config: json!({
                "mineru": {
                    "mode": mineru_mode,
                    "flashBaseUrl": db::MINERU_FLASH_BASE_URL,
                },
            }),
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
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer
                .start_file("result/full.md", options)
                .expect("start full md");
            writer
                .write_all(markdown.as_bytes())
                .expect("write full md");
            writer
                .start_file("result/images/fig.png", options)
                .expect("start image");
            writer.write_all(b"png").expect("write image");
            writer.finish().expect("finish zip");
        }
        bytes.into_inner()
    }

    fn temp_pdf_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "insitu-pdf-parsing-{label}-{}.pdf",
            db::new_id("test")
        ))
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

    fn write_text_response(stream: &mut std::net::TcpStream, body: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/markdown\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write text response");
    }

    #[test]
    fn pdf_parsing_mode_orders_attempts() {
        assert_eq!(
            PdfParsingMode::LocalFirst.attempts(),
            &[PdfParsingAttempt::Local, PdfParsingAttempt::Mineru]
        );
        assert_eq!(
            PdfParsingMode::MineruFirst.attempts(),
            &[PdfParsingAttempt::Mineru, PdfParsingAttempt::Local]
        );
        assert_eq!(
            PdfParsingMode::LocalOnly.attempts(),
            &[PdfParsingAttempt::Local]
        );
        assert_eq!(
            PdfParsingMode::MineruOnly.attempts(),
            &[PdfParsingAttempt::Mineru]
        );
    }

    #[test]
    fn markdown_text_detection_ignores_images_and_ocr_markers() {
        assert!(!markdown_has_extractable_text(
            "---\n\n![Image](assets/task/a.png)\n\n> [OCR REQUIRED - page 1]"
        ));
        assert!(markdown_has_extractable_text(
            "| A | B |\n|---|---|\n| one | two |"
        ));
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
    fn extracts_full_markdown_and_assets_from_mineru_zip() {
        let document = mineru_document_from_zip(br#""#, "task-empty");
        assert!(document.is_err());

        let document =
            mineru_document_from_zip(&test_zip("# full\n\n![Fig](images/fig.png)"), "task-1")
                .expect("document from zip");
        assert_eq!(
            document.markdown,
            "# full\n\n![Fig](assets/task-1/images/fig.png)"
        );
        assert_eq!(document.assets.len(), 1);
        assert_eq!(
            document.assets[0].relative_path,
            "assets/task-1/images/fig.png"
        );
        assert_eq!(document.assets[0].media_type, "image/png");
        assert_eq!(document.assets[0].bytes, b"png");
    }

    #[tokio::test]
    async fn mineru_flash_flow_uploads_polls_and_downloads_markdown() {
        let pdf_path = temp_pdf_path("flash-flow");
        std::fs::write(&pdf_path, b"%PDF-1.4\n% test").expect("write pdf stub");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock mineru flash");
        let address = listener.local_addr().expect("mock address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept upload plan");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("POST /parse/file "));
            assert!(request.contains("\"is_ocr\":true"));
            assert!(request.contains("\"enable_table\":true"));
            assert!(request.contains("\"enable_formula\":true"));
            write_json_response(
                &mut stream,
                &format!(
                    r#"{{"code":0,"data":{{"task_id":"flash-1","file_url":"http://{address}/upload"}}}}"#
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
            assert!(request.starts_with("GET /parse/flash-1 "));
            write_json_response(
                &mut stream,
                &format!(
                    r#"{{"code":0,"data":{{"state":"done","markdown_url":"http://{address}/result.md","extract_progress":{{"total_pages":3}}}}}}"#
                ),
            );

            let (mut stream, _) = listener.accept().expect("accept markdown");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /result.md "));
            write_text_response(&mut stream, "# parsed");
        });

        let config = ProviderRuntimeConfig {
            base_url: format!("http://{address}"),
            config: json!({
                "mineru": {
                    "mode": "flash",
                    "flashBaseUrl": format!("http://{address}"),
                },
            }),
            ..runtime_config(None, "flash")
        };
        let parsed = parse_mineru_flash_pdf(
            &Client::new(),
            &provider(true, "flash"),
            &config,
            "task-flash",
            &pdf_path,
        )
        .await
        .expect("parse through mock mineru flash");

        assert_eq!(parsed.markdown, "# parsed");
        assert_eq!(parsed.engine, PdfParsingEngine::MineruFlash);
        assert_eq!(parsed.page_count, Some(3));
        assert!(parsed.assets.is_empty());

        server.join().expect("mock server");
        let _ = std::fs::remove_file(pdf_path);
    }

    #[test]
    fn rewrites_markdown_image_links_without_touching_regular_links() {
        let mut mapping = HashMap::new();
        mapping.insert("tmp/page1.png".into(), "assets/task/page1.png".into());
        let markdown = "![A](tmp/page1.png)\n[doc](tmp/page1.png)\n";
        assert_eq!(
            rewrite_markdown_image_links(markdown, &mapping),
            "![A](assets/task/page1.png)\n[doc](tmp/page1.png)\n"
        );
    }
}
