use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::placeholders::protect_inline_html;
use super::types::{BlockRef, ParsedChunk};
use super::{chunk_text, DocumentParser};

pub struct HtmlParser;

impl DocumentParser for HtmlParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read HTML source: {error}"))?;
        parse_html_text(
            &text,
            DocumentFormat::Html,
            ContentFormat::Html,
            None,
            input.token_limit,
        )
    }
}

pub fn parse_html_text(
    text: &str,
    format: DocumentFormat,
    content_format: ContentFormat,
    path: Option<String>,
    token_limit: i64,
) -> Result<Vec<ParsedChunk>, String> {
    let parts = chunk_text(text, token_limit);
    parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let mut block_ref = BlockRef::text_block(index);
            block_ref.path = path.clone();
            let (source_text, map_json) =
                protect_inline_html(&part, format, content_format, block_ref)?;
            Ok(ParsedChunk {
                sequence: index as i64,
                preprocessed_text: part,
                source_text,
                map_json,
            })
        })
        .collect()
}
