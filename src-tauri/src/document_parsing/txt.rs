use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderMap};
use super::{chunk_text, chunk_text_with_progress, DocumentParser};

pub struct TxtParser;

impl DocumentParser for TxtParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read TXT source: {error}"))?;
        let parts = match input.progress {
            Some(progress) => chunk_text_with_progress(&text, input.token_limit, Some(progress)),
            None => chunk_text(&text, input.token_limit),
        };
        parts
            .into_iter()
            .enumerate()
            .map(|(index, part)| {
                let map = PlaceholderMap::empty(
                    DocumentFormat::Txt,
                    ContentFormat::PlainText,
                    BlockRef::text_block(index),
                );
                Ok(ParsedChunk {
                    sequence: index as i64,
                    preprocessed_text: part.clone(),
                    source_text: part,
                    map_json: map.to_json()?,
                })
            })
            .collect()
    }
}
