use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::task_prompt::{ContentFormat, DocumentFormat};

pub const PLACEHOLDER_MAP_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ParserInput<'a> {
    pub source_path: &'a Path,
    pub token_limit: i64,
}

#[derive(Debug, Clone)]
pub struct RenderInput<'a> {
    pub source_path: &'a Path,
    pub chunks: &'a [RenderedChunk],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedChunk {
    pub sequence: i64,
    pub preprocessed_text: String,
    pub source_text: String,
    pub map_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderedChunk {
    pub sequence: i64,
    pub source_text: String,
    pub translated_text: String,
    pub map_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaceholderMap {
    pub version: u32,
    pub format: DocumentFormat,
    pub content_format: ContentFormat,
    pub block_ref: BlockRef,
    pub entries: Vec<PlaceholderEntry>,
}

impl PlaceholderMap {
    pub fn empty(
        format: DocumentFormat,
        content_format: ContentFormat,
        block_ref: BlockRef,
    ) -> Self {
        Self {
            version: PLACEHOLDER_MAP_VERSION,
            format,
            content_format,
            block_ref,
            entries: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockRef {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
}

impl BlockRef {
    pub fn whole_document() -> Self {
        Self {
            kind: "document".into(),
            path: None,
            index: None,
            pointer: None,
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub fn text_block(index: usize) -> Self {
        Self {
            kind: "text-block".into(),
            path: None,
            index: Some(index),
            pointer: None,
            prefix: String::new(),
            suffix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaceholderEntry {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub original: String,
    #[serde(default)]
    pub open: String,
    #[serde(default)]
    pub close: String,
    #[serde(default)]
    pub translatable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_ref: Option<String>,
}
