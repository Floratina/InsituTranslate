use regex::Regex;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::placeholders::PlaceholderBuilder;
use super::types::{BlockRef, ParsedChunk};
use super::{chunk_text, DocumentParser};

pub struct MarkdownParser;

impl DocumentParser for MarkdownParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read Markdown source: {error}"))?;
        parse_markdown_text(&text, input.token_limit)
    }
}

pub fn parse_markdown_text(text: &str, token_limit: i64) -> Result<Vec<ParsedChunk>, String> {
    let parts = chunk_text(text, token_limit);
    parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let (source_text, map_json) = protect_markdown_inline(&part, index)?;
            Ok(ParsedChunk {
                sequence: index as i64,
                preprocessed_text: part,
                source_text,
                map_json,
            })
        })
        .collect()
}

fn protect_markdown_inline(text: &str, index: usize) -> Result<(String, String), String> {
    let mut builder = PlaceholderBuilder::new(
        DocumentFormat::Markdown,
        ContentFormat::Markdown,
        BlockRef::text_block(index),
    );
    let mut source = text.to_string();
    for (kind, pattern, open, close) in [
        ("strong", r"\*\*(.+?)\*\*", "**", "**"),
        ("emphasis", r"\*(.+?)\*", "*", "*"),
        ("code", r"`([^`]+?)`", "`", "`"),
    ] {
        let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
        loop {
            let Some(captures) = regex.captures(&source) else {
                break;
            };
            let Some(full) = captures.get(0) else {
                break;
            };
            let Some(inner) = captures.get(1) else {
                break;
            };
            let id = builder.wrap(kind, open, close);
            let replacement = format!("<{id}>{}</{id}>", inner.as_str());
            source.replace_range(full.range(), &replacement);
        }
    }
    let link = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").map_err(|error| error.to_string())?;
    loop {
        let Some(captures) = link.captures(&source) else {
            break;
        };
        let Some(full) = captures.get(0) else {
            break;
        };
        let label = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let target = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let id = builder.wrap("link", "[", format!("]({target})"));
        let replacement = format!("<{id}>{label}</{id}>");
        source.replace_range(full.range(), &replacement);
    }
    Ok((source, builder.map().to_json()?))
}
