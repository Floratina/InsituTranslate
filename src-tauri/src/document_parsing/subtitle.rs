use regex::Regex;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderMap};
use super::DocumentParser;

pub struct SubtitleParser {
    pub format: DocumentFormat,
    pub content_format: ContentFormat,
}

impl DocumentParser for SubtitleParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let _ = self.content_format;
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read subtitle source: {error}"))?;
        match self.format {
            DocumentFormat::Srt => parse_srt(&text),
            DocumentFormat::Ass => parse_ass(&text),
            DocumentFormat::Lrc => parse_lrc(&text),
            _ => Err("Unsupported subtitle parser format".into()),
        }
    }
}

fn parse_srt(text: &str) -> Result<Vec<ParsedChunk>, String> {
    let blocks = text.split("\n\n").collect::<Vec<_>>();
    blocks
        .into_iter()
        .enumerate()
        .filter_map(|(index, block)| {
            let lines = block.lines().collect::<Vec<_>>();
            if lines.len() < 3 {
                return None;
            }
            let prefix = format!("{}\n{}\n", lines[0], lines[1]);
            let body = lines[2..].join("\n");
            Some(chunk_with_shell(
                index,
                DocumentFormat::Srt,
                ContentFormat::Srt,
                prefix,
                if text.ends_with("\n\n") {
                    "\n\n"
                } else {
                    "\n\n"
                }
                .to_string(),
                body,
            ))
        })
        .collect()
}

fn parse_ass(text: &str) -> Result<Vec<ParsedChunk>, String> {
    let mut chunks = Vec::new();
    let mut header = Vec::new();
    for line in text.lines() {
        if line.starts_with("Dialogue:") {
            let fields = line.splitn(10, ',').collect::<Vec<_>>();
            if fields.len() == 10 {
                let prefix = format!("{},", fields[..9].join(","));
                chunks.push(chunk_with_shell(
                    chunks.len(),
                    DocumentFormat::Ass,
                    ContentFormat::Ass,
                    prefix,
                    "\n".into(),
                    fields[9].to_string(),
                )?);
            }
        } else {
            header.push(line);
        }
    }
    if let Some(first) = chunks.first_mut() {
        let map = super::parse_map(&first.map_json)?;
        let mut block_ref = map.block_ref;
        block_ref.prefix = format!("{}\n{}", header.join("\n"), block_ref.prefix);
        let mut map = PlaceholderMap::empty(DocumentFormat::Ass, ContentFormat::Ass, block_ref);
        map.entries = super::parse_map(&first.map_json)?.entries;
        first.map_json = map.to_json()?;
    }
    Ok(chunks)
}

fn parse_lrc(text: &str) -> Result<Vec<ParsedChunk>, String> {
    let regex = Regex::new(r"^((?:\[[^\]]+\])+)(.*)$").map_err(|error| error.to_string())?;
    let mut chunks = Vec::new();
    for line in text.lines() {
        let Some(captures) = regex.captures(line) else {
            continue;
        };
        let prefix = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or_default();
        let body = captures
            .get(2)
            .map(|value| value.as_str())
            .unwrap_or_default();
        chunks.push(chunk_with_shell(
            chunks.len(),
            DocumentFormat::Lrc,
            ContentFormat::Lrc,
            prefix.to_string(),
            "\n".into(),
            body.to_string(),
        )?);
    }
    Ok(chunks)
}

fn chunk_with_shell(
    index: usize,
    format: DocumentFormat,
    content_format: ContentFormat,
    prefix: String,
    suffix: String,
    body: String,
) -> Result<ParsedChunk, String> {
    let map = PlaceholderMap::empty(
        format,
        content_format,
        BlockRef {
            kind: "timed-text".into(),
            path: None,
            index: Some(index),
            pointer: None,
            prefix,
            suffix,
        },
    );
    Ok(ParsedChunk {
        sequence: index as i64,
        preprocessed_text: format!("{}{}{}", map.block_ref.prefix, body, map.block_ref.suffix),
        source_text: body,
        map_json: map.to_json()?,
    })
}
