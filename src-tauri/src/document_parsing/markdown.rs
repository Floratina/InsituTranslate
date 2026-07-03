use super::placeholders::protect_markdown;
use super::types::{BlockRef, ParsedChunk, ParserProgress};
use super::{chunk_raw_blocks_with_progress, token_limit_usize, DocumentParser, RawBlock};

pub struct MarkdownParser;

impl DocumentParser for MarkdownParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read Markdown source: {error}"))?;
        match input.progress {
            Some(progress) => {
                parse_markdown_text_with_progress(&text, input.token_limit, Some(progress))
            }
            None => parse_markdown_text(&text, input.token_limit),
        }
    }
}

pub fn parse_markdown_text(text: &str, token_limit: i64) -> Result<Vec<ParsedChunk>, String> {
    parse_markdown_text_with_progress(text, token_limit, None)
}

pub fn parse_markdown_text_with_progress(
    text: &str,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let parts = chunk_raw_blocks_with_progress(
        markdown_raw_blocks(text),
        token_limit_usize(token_limit),
        progress,
    );
    parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let (source_text, map_json) = protect_markdown(&part, BlockRef::text_block(index))?;
            Ok(ParsedChunk {
                sequence: index as i64,
                preprocessed_text: part,
                source_text,
                map_json,
            })
        })
        .collect()
}

fn markdown_raw_blocks(text: &str) -> Vec<RawBlock> {
    if text.is_empty() {
        return vec![RawBlock::new(String::new(), true)];
    }

    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut in_code_fence = false;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let starts_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        current.push_str(line);

        if starts_fence {
            in_code_fence = !in_code_fence;
            if !in_code_fence {
                push_markdown_raw_block(&mut blocks, &mut current);
            }
            continue;
        }

        if in_code_fence {
            continue;
        }

        if line.trim().is_empty() {
            push_markdown_raw_block(&mut blocks, &mut current);
        }
    }

    push_markdown_raw_block(&mut blocks, &mut current);
    if blocks.is_empty() {
        blocks.push(RawBlock::new(String::new(), true));
    }
    blocks
}

fn push_markdown_raw_block(blocks: &mut Vec<RawBlock>, current: &mut String) {
    if current.is_empty() {
        return;
    }
    let text = std::mem::take(current);
    let is_breakable = !is_markdown_format_sensitive(&text);
    blocks.push(RawBlock::new(text, is_breakable));
}

fn is_markdown_format_sensitive(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return true;
    }
    if text.lines().any(is_markdown_structural_line) {
        return true;
    }
    contains_markdown_inline_markup(text)
}

fn is_markdown_structural_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("> ") || trimmed.starts_with('>') {
        return true;
    }
    if trimmed.starts_with('#') || trimmed.starts_with('|') {
        return true;
    }
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("- [")
        || trimmed.starts_with("* [")
    {
        return true;
    }
    let Some((number, rest)) = trimmed.split_once('.') else {
        return false;
    };
    !number.is_empty()
        && number.chars().all(|character| character.is_ascii_digit())
        && rest.starts_with(' ')
}

fn contains_markdown_inline_markup(text: &str) -> bool {
    text.contains('`')
        || text.contains("](")
        || text.contains("![")
        || text.contains("**")
        || text.contains("__")
        || text.contains("~~")
        || text.contains('*')
        || text.contains('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_markdown_writes_placeholders_to_source_text_and_preserves_preprocessed_text() {
        let text = "Intro **bold** and [docs](https://example.com)\n";
        let chunks = parse_markdown_text(text, 800).expect("parse markdown");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].preprocessed_text, text);
        assert_eq!(
            chunks[0].source_text,
            "Intro <t1>bold</t1> and <t2>docs</t2>\n"
        );
        assert_ne!(chunks[0].source_text, chunks[0].preprocessed_text);
    }

    #[test]
    fn markdown_format_sensitive_block_is_not_split_before_placeholder_protection() {
        let text = "**bold** ".repeat(80);
        let chunks = parse_markdown_text(&text, 5).expect("parse markdown");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].preprocessed_text, text);
        assert!(chunks[0].source_text.contains("<t1>"));
    }
}
