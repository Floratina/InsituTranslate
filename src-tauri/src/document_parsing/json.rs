use serde_json::Value;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{
    BlockRef, ParsedChunk, ParserProgress, ParserProgressStage, PlaceholderMap, RenderInput,
};
use super::DocumentParser;

pub struct JsonParser;

impl DocumentParser for JsonParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read JSON source: {error}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("Unable to parse JSON source: {error}"))?;
        let mut chunks = Vec::new();
        let total_strings = count_strings(&value);
        let mut progress = input.progress;
        emit_json_progress(&mut progress, 0, total_strings);
        collect_strings(&value, "", &mut chunks, &mut progress, total_strings)?;
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
    progress: &mut Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
    total: u64,
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
            emit_json_progress(progress, chunks.len() as u64, total);
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_strings(item, &format!("{pointer}/{index}"), chunks, progress, total)?;
            }
        }
        Value::Object(object) => {
            for (key, item) in object {
                collect_strings(
                    item,
                    &format!("{pointer}/{}", escape_pointer(key)),
                    chunks,
                    progress,
                    total,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn count_strings(value: &Value) -> u64 {
    match value {
        Value::String(_) => 1,
        Value::Array(items) => items.iter().map(count_strings).sum(),
        Value::Object(object) => object.values().map(count_strings).sum(),
        _ => 0,
    }
}

fn emit_json_progress(
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

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
