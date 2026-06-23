mod docx;
mod epub;
mod html;
mod json;
mod markdown;
mod pdf;
pub mod placeholders;
mod subtitle;
mod tag_corrector;
mod txt;
pub mod types;
mod xlsx;

use std::path::Path;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use self::docx::DocxParser;
use self::epub::EpubParser;
use self::html::HtmlParser;
use self::json::JsonParser;
use self::markdown::MarkdownParser;
use self::pdf::PdfParser;
use self::placeholders::restore_from_json;
use self::subtitle::SubtitleParser;
use self::txt::TxtParser;
use self::types::{ParsedChunk, ParserInput, PlaceholderMap, RenderInput, RenderedChunk};
use self::xlsx::XlsxParser;

pub trait DocumentParser {
    fn parse(&self, input: ParserInput<'_>) -> Result<Vec<ParsedChunk>, String>;

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        restore_from_json(map_json, after_translate_text)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        Ok(input
            .chunks
            .iter()
            .map(|chunk| chunk.translated_text.as_str())
            .collect::<String>()
            .into_bytes())
    }
}

pub fn parse_source_file(
    _task_id: &str,
    source_path: &Path,
    token_limit: i64,
) -> Result<Vec<ParsedChunk>, String> {
    let parser = parser_for_path(source_path)?;
    let mut chunks = parser.parse(ParserInput {
        source_path,
        token_limit,
    })?;
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
    }
    if chunks.is_empty() {
        let map = PlaceholderMap::empty(
            document_format_from_path(source_path)?,
            content_format_from_path(source_path)?,
            types::BlockRef::whole_document(),
        );
        chunks.push(ParsedChunk {
            sequence: 0,
            preprocessed_text: String::new(),
            source_text: String::new(),
            map_json: map.to_json()?,
        });
    }
    Ok(chunks)
}

pub fn restore_chunk_for_map(map_json: &str, after_translate_text: &str) -> Result<String, String> {
    let map = parse_map(map_json)?;
    let parser = parser_for_format(map.format, map.content_format);
    parser.restore_chunk(map_json, after_translate_text)
}

pub fn render_translated_document(
    source_path: &Path,
    chunks: &[RenderedChunk],
) -> Result<Vec<u8>, String> {
    let parser = parser_for_path(source_path)?;
    parser.render_document(RenderInput {
        source_path,
        chunks,
    })
}

pub fn parse_map(map_json: &str) -> Result<PlaceholderMap, String> {
    if map_json.trim().is_empty() || map_json.trim() == "{}" {
        return Ok(PlaceholderMap::empty(
            DocumentFormat::Txt,
            ContentFormat::PlainText,
            types::BlockRef::whole_document(),
        ));
    }
    serde_json::from_str(map_json).map_err(|error| format!("Invalid placeholder map JSON: {error}"))
}

pub fn document_format_from_path(path: &Path) -> Result<DocumentFormat, String> {
    match extension(path).as_str() {
        "pdf" => Ok(DocumentFormat::Pdf),
        "md" => Ok(DocumentFormat::Markdown),
        "epub" => Ok(DocumentFormat::Epub),
        "html" | "htm" => Ok(DocumentFormat::Html),
        "txt" => Ok(DocumentFormat::Txt),
        "docx" => Ok(DocumentFormat::Docx),
        "xlsx" => Ok(DocumentFormat::Xlsx),
        "json" => Ok(DocumentFormat::Json),
        "srt" => Ok(DocumentFormat::Srt),
        "ass" => Ok(DocumentFormat::Ass),
        "lrc" => Ok(DocumentFormat::Lrc),
        _ => Err("Unsupported source document format".into()),
    }
}

pub fn content_format_from_path(path: &Path) -> Result<ContentFormat, String> {
    match extension(path).as_str() {
        "pdf" | "md" => Ok(ContentFormat::Markdown),
        "epub" => Ok(ContentFormat::Xhtml),
        "html" | "htm" => Ok(ContentFormat::Html),
        "json" => Ok(ContentFormat::Json),
        "txt" | "docx" | "xlsx" => Ok(ContentFormat::PlainText),
        "srt" => Ok(ContentFormat::Srt),
        "ass" => Ok(ContentFormat::Ass),
        "lrc" => Ok(ContentFormat::Lrc),
        _ => Err("Unsupported source content format".into()),
    }
}

pub fn supported_source_file(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "pdf"
            | "md"
            | "epub"
            | "html"
            | "htm"
            | "txt"
            | "docx"
            | "xlsx"
            | "json"
            | "srt"
            | "ass"
            | "lrc"
    )
}

pub fn source_extension(path: &str) -> Result<&'static str, String> {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => Ok("md"),
        "md" => Ok("md"),
        "epub" => Ok("epub"),
        "html" => Ok("html"),
        "htm" => Ok("htm"),
        "txt" => Ok("txt"),
        "docx" => Ok("docx"),
        "xlsx" => Ok("xlsx"),
        "json" => Ok("json"),
        "srt" => Ok("srt"),
        "ass" => Ok("ass"),
        "lrc" => Ok("lrc"),
        _ => Err("Unsupported source document format".into()),
    }
}

pub fn chunk_text(text: &str, token_limit: i64) -> Vec<String> {
    let token_limit = token_limit.max(1) as u64;
    let max_chars = (token_limit * 4).max(200) as usize;
    let mut chunks = Vec::new();
    let mut current = String::new();
    for segment in text.split_inclusive('\n') {
        if !current.is_empty() && estimate_tokens(&current) + estimate_tokens(segment) > token_limit
        {
            chunks.push(std::mem::take(&mut current));
        }
        if estimate_tokens(segment) > token_limit {
            for part in split_long_segment(segment, max_chars) {
                if current.is_empty() {
                    chunks.push(part);
                } else {
                    chunks.push(std::mem::take(&mut current));
                    chunks.push(part);
                }
            }
        } else {
            current.push_str(segment);
        }
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_long_segment(segment: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for character in segment.chars() {
        current.push(character);
        if current.len() >= max_chars {
            parts.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in text.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    (ascii + 3) / 4 + non_ascii
}

fn parser_for_path(path: &Path) -> Result<Box<dyn DocumentParser + Send + Sync>, String> {
    match extension(path).as_str() {
        "pdf" => Ok(Box::new(PdfParser)),
        "md" => Ok(Box::new(MarkdownParser)),
        "epub" => Ok(Box::new(EpubParser)),
        "html" | "htm" => Ok(Box::new(HtmlParser)),
        "txt" => Ok(Box::new(TxtParser)),
        "docx" => Ok(Box::new(DocxParser)),
        "xlsx" => Ok(Box::new(XlsxParser)),
        "json" => Ok(Box::new(JsonParser)),
        "srt" => Ok(Box::new(SubtitleParser {
            format: DocumentFormat::Srt,
            content_format: ContentFormat::Srt,
        })),
        "ass" => Ok(Box::new(SubtitleParser {
            format: DocumentFormat::Ass,
            content_format: ContentFormat::Ass,
        })),
        "lrc" => Ok(Box::new(SubtitleParser {
            format: DocumentFormat::Lrc,
            content_format: ContentFormat::Lrc,
        })),
        _ => Err("Unsupported source document format".into()),
    }
}

fn parser_for_format(
    format: DocumentFormat,
    content_format: ContentFormat,
) -> Box<dyn DocumentParser + Send + Sync> {
    match format {
        DocumentFormat::Pdf => Box::new(PdfParser),
        DocumentFormat::Markdown => Box::new(MarkdownParser),
        DocumentFormat::Epub => Box::new(EpubParser),
        DocumentFormat::Html => Box::new(HtmlParser),
        DocumentFormat::Txt => Box::new(TxtParser),
        DocumentFormat::Json => Box::new(JsonParser),
        DocumentFormat::Docx => Box::new(DocxParser),
        DocumentFormat::Xlsx => Box::new(XlsxParser),
        DocumentFormat::Srt | DocumentFormat::Ass | DocumentFormat::Lrc => {
            Box::new(SubtitleParser {
                format,
                content_format,
            })
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}
