use regex::Regex;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::tag_corrector::correct_and_restore;
use super::types::{BlockRef, PlaceholderEntry, PlaceholderMap};

#[derive(Debug)]
pub struct PlaceholderBuilder {
    map: PlaceholderMap,
    next_index: usize,
}

impl PlaceholderBuilder {
    pub fn new(format: DocumentFormat, content_format: ContentFormat, block_ref: BlockRef) -> Self {
        Self {
            map: PlaceholderMap::empty(format, content_format, block_ref),
            next_index: 1,
        }
    }

    pub fn wrap(
        &mut self,
        kind: &str,
        open: impl Into<String>,
        close: impl Into<String>,
    ) -> String {
        let id = self.next_id();
        self.map.entries.push(PlaceholderEntry {
            id: id.clone(),
            kind: kind.into(),
            original: String::new(),
            open: open.into(),
            close: close.into(),
            translatable: true,
            native_ref: None,
        });
        id
    }

    pub fn map(self) -> PlaceholderMap {
        self.map
    }

    fn next_id(&mut self) -> String {
        loop {
            let id = format!("t{}", self.next_index);
            self.next_index += 1;
            return id;
        }
    }
}

pub fn restore_from_json(map_json: &str, after_translate_text: &str) -> Result<String, String> {
    let map: PlaceholderMap = if map_json.trim().is_empty() || map_json.trim() == "{}" {
        return Ok(after_translate_text.to_string());
    } else {
        serde_json::from_str(map_json)
            .map_err(|error| format!("Invalid placeholder map JSON: {error}"))?
    };
    restore_with_map(&map, after_translate_text)
}

pub fn restore_with_map(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    Ok(correct_and_restore(after_translate_text, map))
}

pub fn protect_inline_html(
    text: &str,
    format: DocumentFormat,
    content_format: ContentFormat,
    block_ref: BlockRef,
) -> Result<(String, String), String> {
    let mut builder = PlaceholderBuilder::new(format, content_format, block_ref);
    let tag = Regex::new(r"(?is)<([a-z][a-z0-9:_-]*)(?:\s[^>]*)?>(.*?)</([a-z][a-z0-9:_-]*)>")
        .map_err(|error| error.to_string())?;
    let mut source = text.to_string();
    loop {
        let Some(captures) = tag.captures(&source) else {
            break;
        };
        let Some(full) = captures.get(0) else {
            break;
        };
        let Some(name) = captures.get(1) else {
            break;
        };
        let Some(inner) = captures.get(2) else {
            break;
        };
        let Some(close_name) = captures.get(3) else {
            break;
        };
        if !name.as_str().eq_ignore_ascii_case(close_name.as_str()) {
            break;
        }
        let full_text = full.as_str().to_string();
        let open_end = full_text.find('>').map(|index| index + 1).unwrap_or(0);
        let open = full_text[..open_end].to_string();
        let close = format!("</{}>", name.as_str());
        let id = builder.wrap("markup", open, close);
        let replacement = format!("<{id}>{}</{id}>", inner.as_str());
        source.replace_range(full.range(), &replacement);
    }
    let map_json = builder.map().to_json()?;
    Ok((source, map_json))
}

#[cfg(test)]
mod tests {
    use crate::task_prompt::{ContentFormat, DocumentFormat};

    use super::*;
    use crate::document_parsing::types::BlockRef;

    #[test]
    fn restores_wrapped_placeholder() {
        let mut builder = PlaceholderBuilder::new(
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        );
        let id = builder.wrap("html", "<strong>", "</strong>");
        let map = builder.map();
        let restored = restore_with_map(&map, &format!("点 <{id}>这里</{id}>")).unwrap();
        assert_eq!(restored, "点 <strong>这里</strong>");
    }

    #[test]
    fn falls_back_to_plain_text_for_missing_placeholder() {
        let mut builder = PlaceholderBuilder::new(
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        );
        builder.wrap("html", "<strong>", "</strong>");
        let restored = restore_with_map(&builder.map(), "没有标签").unwrap();
        assert_eq!(restored, "没有标签");
    }
}
