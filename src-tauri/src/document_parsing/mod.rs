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
use std::sync::OnceLock;

use crate::task_prompt::{ContentFormat, DocumentFormat};
use text_splitter::{ChunkConfig, TextSplitter};
use tiktoken_rs::{cl100k_base, CoreBPE};
use unicode_segmentation::UnicodeSegmentation;

use self::docx::DocxParser;
use self::epub::EpubParser;
use self::html::HtmlParser;
use self::json::JsonParser;
use self::markdown::MarkdownParser;
use self::pdf::PdfParser;
use self::placeholders::restore_from_json;
use self::subtitle::SubtitleParser;
use self::txt::TxtParser;
use self::types::{
    ParsedChunk, ParserInput, ParserProgress, ParserProgressStage, PlaceholderMap, RenderInput,
    RenderedChunk,
};
use self::xlsx::XlsxParser;

pub const HARD_CHUNK_TOKEN_LIMIT: usize = 2000;

static CL100K_TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();

pub trait DocumentParser {
    fn parse(&self, input: ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String>;

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
    parse_source_file_with_progress(_task_id, source_path, token_limit, None)
}

pub fn parse_source_file_with_progress<'path, 'progress>(
    _task_id: &str,
    source_path: &'path Path,
    token_limit: i64,
    progress: Option<&'progress mut (dyn FnMut(ParserProgress) + Send + 'progress)>,
) -> Result<Vec<ParsedChunk>, String> {
    let parser = parser_for_path(source_path)?;
    let mut chunks = parser.parse(ParserInput {
        source_path,
        token_limit,
        progress,
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

pub fn parse_pdf_markdown_text(text: &str, token_limit: i64) -> Result<Vec<ParsedChunk>, String> {
    parse_pdf_markdown_text_with_progress(text, token_limit, None)
}

pub fn parse_pdf_markdown_text_with_progress<'progress>(
    text: &str,
    token_limit: i64,
    progress: Option<&'progress mut (dyn FnMut(ParserProgress) + Send + 'progress)>,
) -> Result<Vec<ParsedChunk>, String> {
    let mut chunks = markdown::parse_markdown_text_with_progress(text, token_limit, progress)?;
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
        let mut map = parse_map(&chunk.map_json)?;
        map.format = DocumentFormat::Pdf;
        map.content_format = ContentFormat::Markdown;
        chunk.map_json = map.to_json()?;
    }
    if chunks.is_empty() {
        let map = PlaceholderMap::empty(
            DocumentFormat::Pdf,
            ContentFormat::Markdown,
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
        "txt" => Ok(ContentFormat::PlainText),
        "docx" | "xlsx" => Ok(ContentFormat::Xml),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    pub text: String,
    pub is_breakable: bool,
}

impl RawBlock {
    pub fn new(text: impl Into<String>, is_breakable: bool) -> Self {
        Self {
            text: text.into(),
            is_breakable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlockRef<T> {
    pub text: String,
    pub is_breakable: bool,
    pub metadata: T,
}

impl<T> RawBlockRef<T> {
    pub fn new(text: impl Into<String>, is_breakable: bool, metadata: T) -> Self {
        Self {
            text: text.into(),
            is_breakable,
            metadata,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedRawBlock<T> {
    pub text: String,
    pub metadata: T,
    pub source_start: usize,
    pub source_end: usize,
}

pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    tokenizer().encode_ordinary(text).len()
}

pub fn chunk_text(text: &str, token_limit: i64) -> Vec<String> {
    chunk_raw_blocks(
        vec![RawBlock::new(text.to_string(), true)],
        token_limit_usize(token_limit),
    )
}

pub fn chunk_text_with_progress(
    text: &str,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Vec<String> {
    chunk_raw_blocks_with_progress(
        vec![RawBlock::new(text.to_string(), true)],
        token_limit_usize(token_limit),
        progress,
    )
}

pub fn chunk_raw_blocks(blocks: Vec<RawBlock>, max_tokens: usize) -> Vec<String> {
    chunk_raw_blocks_with_progress(blocks, max_tokens, None)
}

pub fn chunk_raw_blocks_with_progress(
    blocks: Vec<RawBlock>,
    max_tokens: usize,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Vec<String> {
    chunk_raw_block_refs_with_progress(
        blocks
            .into_iter()
            .map(|block| RawBlockRef::new(block.text, block.is_breakable, ()))
            .collect(),
        max_tokens,
        progress,
    )
    .into_iter()
    .map(|chunk| {
        chunk
            .into_iter()
            .map(|block| block.text)
            .collect::<String>()
    })
    .collect()
}

pub fn chunk_raw_block_refs<T: Clone>(
    blocks: Vec<RawBlockRef<T>>,
    max_tokens: usize,
) -> Vec<Vec<ChunkedRawBlock<T>>> {
    chunk_raw_block_refs_with_progress(blocks, max_tokens, None)
}

pub fn chunk_raw_block_refs_with_progress<T: Clone>(
    blocks: Vec<RawBlockRef<T>>,
    max_tokens: usize,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Vec<Vec<ChunkedRawBlock<T>>> {
    let max_tokens = max_tokens.max(1);
    let total_blocks = blocks.len() as u64;
    let mut progress = progress;
    emit_chunking_progress(&mut progress, 0, total_blocks);
    let mut chunks = Vec::new();
    let mut current = Vec::<ChunkedRawBlock<T>>::new();
    let mut current_tokens = 0_usize;

    for (index, block) in blocks.into_iter().enumerate() {
        let block_tokens = count_tokens(&block.text);
        if block.is_breakable && block_tokens > max_tokens {
            flush_chunk(&mut chunks, &mut current, &mut current_tokens);
            for (part, start, end) in split_breakable_text(&block.text, max_tokens) {
                push_chunked_block(
                    &mut chunks,
                    &mut current,
                    &mut current_tokens,
                    max_tokens,
                    ChunkedRawBlock {
                        text: part,
                        metadata: block.metadata.clone(),
                        source_start: start,
                        source_end: end,
                    },
                    true,
                );
            }
            emit_chunking_progress(&mut progress, index as u64 + 1, total_blocks);
            continue;
        }

        let block_len = block.text.len();
        if !block.is_breakable && block_tokens > HARD_CHUNK_TOKEN_LIMIT {
            flush_chunk(&mut chunks, &mut current, &mut current_tokens);
            current.push(ChunkedRawBlock {
                text: block.text,
                metadata: block.metadata,
                source_start: 0,
                source_end: block_len,
            });
            flush_chunk(&mut chunks, &mut current, &mut current_tokens);
            emit_chunking_progress(&mut progress, index as u64 + 1, total_blocks);
            continue;
        }

        let force_own_chunk = !block.is_breakable && block_tokens > max_tokens;
        push_chunked_block(
            &mut chunks,
            &mut current,
            &mut current_tokens,
            max_tokens,
            ChunkedRawBlock {
                text: block.text,
                metadata: block.metadata,
                source_start: 0,
                source_end: block_len,
            },
            force_own_chunk,
        );
        if force_own_chunk {
            flush_chunk(&mut chunks, &mut current, &mut current_tokens);
        }
        emit_chunking_progress(&mut progress, index as u64 + 1, total_blocks);
    }

    flush_chunk(&mut chunks, &mut current, &mut current_tokens);
    if chunks.is_empty() {
        chunks.push(Vec::new());
    }
    chunks
}

fn emit_chunking_progress(
    progress: &mut Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
    current: u64,
    total: u64,
) {
    if total == 0 {
        return;
    }
    if let Some(progress) = progress.as_deref_mut() {
        progress(ParserProgress {
            stage: ParserProgressStage::Chunking,
            current,
            total,
            label: format!("分块 ({current}/{total})"),
        });
    }
}

pub fn token_limit_usize(token_limit: i64) -> usize {
    token_limit.max(1) as usize
}

fn tokenizer() -> &'static CoreBPE {
    CL100K_TOKENIZER.get_or_init(|| {
        cl100k_base().expect("cl100k_base tokenizer should be available in tiktoken-rs")
    })
}

fn push_chunked_block<T>(
    chunks: &mut Vec<Vec<ChunkedRawBlock<T>>>,
    current: &mut Vec<ChunkedRawBlock<T>>,
    current_tokens: &mut usize,
    max_tokens: usize,
    block: ChunkedRawBlock<T>,
    force_own_chunk: bool,
) {
    let block_tokens = count_tokens(&block.text);
    if !current.is_empty() && (*current_tokens + block_tokens > max_tokens || force_own_chunk) {
        flush_chunk(chunks, current, current_tokens);
    }
    *current_tokens += block_tokens;
    current.push(block);
}

fn flush_chunk<T>(
    chunks: &mut Vec<Vec<ChunkedRawBlock<T>>>,
    current: &mut Vec<ChunkedRawBlock<T>>,
    current_tokens: &mut usize,
) {
    if current.is_empty() {
        return;
    }
    chunks.push(std::mem::take(current));
    *current_tokens = 0;
}

fn split_breakable_text(text: &str, max_tokens: usize) -> Vec<(String, usize, usize)> {
    let max_tokens = max_tokens.max(1);
    let config = ChunkConfig::new(max_tokens).with_sizer(tokenizer().clone());
    let splitter = TextSplitter::new(config);
    let mut chunks = Vec::new();
    let mut cursor = 0_usize;

    for chunk in splitter.chunks(text) {
        if chunk.is_empty() {
            continue;
        }
        let chunk_end = text[cursor..]
            .find(chunk)
            .map(|offset| cursor + offset + chunk.len())
            .unwrap_or_else(|| cursor + chunk.len().min(text.len().saturating_sub(cursor)));
        let preserved = &text[cursor..chunk_end];
        if count_tokens(preserved) > max_tokens {
            chunks.extend(split_grapheme_safe(preserved, max_tokens, cursor));
        } else if !preserved.is_empty() {
            chunks.push((preserved.to_string(), cursor, chunk_end));
        }
        cursor = chunk_end;
    }

    if cursor < text.len() {
        let tail = &text[cursor..];
        if count_tokens(tail) > max_tokens {
            chunks.extend(split_grapheme_safe(tail, max_tokens, cursor));
        } else if !tail.is_empty() {
            chunks.push((tail.to_string(), cursor, text.len()));
        }
    }

    if chunks.is_empty() && !text.is_empty() {
        chunks.push((text.to_string(), 0, text.len()));
    }
    chunks
}

fn split_grapheme_safe(
    text: &str,
    max_tokens: usize,
    base_offset: usize,
) -> Vec<(String, usize, usize)> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_start = 0_usize;
    let mut current_end = 0_usize;

    for (index, grapheme) in text.grapheme_indices(true) {
        let next_end = index + grapheme.len();
        let candidate_tokens = count_tokens(&current) + count_tokens(grapheme);
        if !current.is_empty() && candidate_tokens > max_tokens {
            chunks.push((
                std::mem::take(&mut current),
                base_offset + current_start,
                base_offset + current_end,
            ));
            current_start = index;
        }
        current.push_str(grapheme);
        current_end = next_end;
    }

    if !current.is_empty() {
        chunks.push((
            current,
            base_offset + current_start,
            base_offset + current_end,
        ));
    }
    chunks
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_raw_blocks_splits_breakable_plain_text_with_tiktoken_sizer() {
        let text = "Alpha beta gamma delta. ".repeat(40);
        let chunks = chunk_raw_blocks(vec![RawBlock::new(text.clone(), true)], 12);

        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|chunk| count_tokens(chunk) <= 12));
    }

    #[test]
    fn chunk_raw_blocks_keeps_format_sensitive_block_whole() {
        let text = "**bold** ".repeat(80);
        let chunks = chunk_raw_blocks(vec![RawBlock::new(text.clone(), false)], 5);

        assert_eq!(chunks, vec![text]);
    }
}
