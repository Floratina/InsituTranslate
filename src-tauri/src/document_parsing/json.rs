use serde_json::Value;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderMap, RenderInput};
use super::DocumentParser;

pub struct JsonParser;

impl DocumentParser for JsonParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read JSON source: {error}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("Unable to parse JSON source: {error}"))?;
        let mut chunks = Vec::new();
        collect_strings(&value, "", &mut chunks)?;
        for (index, chunk) in chunks.iter_mut().enumerate() {
            chunk.sequence = index as i64;
        }
        Ok(chunks)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read JSON source for render: {error}"))?;
        let mut value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("Unable to parse JSON source for render: {error}"))?;
        for chunk in input.chunks {
            let map = super::parse_map(&chunk.map_json)?;
            if let Some(pointer) = map.block_ref.pointer {
                if let Some(target) = value.pointer_mut(&pointer) {
                    *target = Value::String(chunk.translated_text.clone());
                }
            }
        }
        serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())
    }
}

fn collect_strings(
    value: &Value,
    pointer: &str,
    chunks: &mut Vec<ParsedChunk>,
) -> Result<(), String> {
    match value {
        Value::String(text) => {
            let map = PlaceholderMap::empty(
                DocumentFormat::Json,
                ContentFormat::Json,
                BlockRef {
                    kind: "json-value".into(),
                    path: None,
                    index: None,
                    pointer: Some(pointer.to_string()),
                    prefix: String::new(),
                    suffix: String::new(),
                },
            );
            chunks.push(ParsedChunk {
                sequence: 0,
                preprocessed_text: text.clone(),
                source_text: text.clone(),
                map_json: map.to_json()?,
            });
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_strings(item, &format!("{pointer}/{index}"), chunks)?;
            }
        }
        Value::Object(object) => {
            for (key, item) in object {
                collect_strings(item, &format!("{pointer}/{}", escape_pointer(key)), chunks)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
