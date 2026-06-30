use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::placeholders::protect_html;
use super::types::{BlockRef, ParsedChunk, RenderInput, RenderedChunk};
use super::{chunk_raw_block_refs, token_limit_usize, DocumentParser, RawBlockRef};

const EPUB_TEXT_KIND: &str = "epub-xhtml-text";
const EPUB_ATTRIBUTE_KIND: &str = "epub-xhtml-attribute";
const EPUB_LEGACY_HTML_TEXT_KIND: &str = "html-text";
const EPUB_LEGACY_HTML_ATTRIBUTE_KIND: &str = "html-attribute";
const EPUB_LEGACY_TEXT_BLOCK_KIND: &str = "text-block";
const TRANSLATABLE_ATTRIBUTES: &[&str] = &["alt", "title", "placeholder"];

pub struct EpubParser;

impl DocumentParser for EpubParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let file = File::open(input.source_path)
            .map_err(|error| format!("Unable to open EPUB source: {error}"))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("Unable to read EPUB archive: {error}"))?;
        let mut chunks = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().replace('\\', "/");
            if !is_html_entry(&name) {
                continue;
            }
            let mut text = String::new();
            file.read_to_string(&mut text)
                .map_err(|error| format!("Unable to read EPUB HTML entry {name}: {error}"))?;
            let mut parsed = parse_epub_xhtml_text(&text, name, input.token_limit)?;
            chunks.append(&mut parsed);
        }
        resequence(&mut chunks);
        Ok(chunks)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(input.source_path)
            .map_err(|error| format!("Unable to read EPUB for render: {error}"))?;
        let reader = Cursor::new(bytes);
        let mut archive = ZipArchive::new(reader)
            .map_err(|error| format!("Unable to open EPUB for render: {error}"))?;
        let mut output = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut output);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().to_string();
            writer
                .start_file(&name, options)
                .map_err(|error| error.to_string())?;
            if is_html_entry(&name) {
                let mut text = String::new();
                file.read_to_string(&mut text)
                    .map_err(|error| error.to_string())?;
                let replacement = render_html_entry(&name, text, input.chunks)?;
                writer
                    .write_all(replacement.as_bytes())
                    .map_err(|error| error.to_string())?;
            } else {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                writer
                    .write_all(&bytes)
                    .map_err(|error| error.to_string())?;
            }
        }
        writer.finish().map_err(|error| error.to_string())?;
        Ok(output.into_inner())
    }
}

fn is_html_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")
}

fn render_html_entry(
    name: &str,
    original_text: String,
    chunks: &[RenderedChunk],
) -> Result<String, String> {
    render_epub_xhtml_entry(name, &original_text, chunks)
}

fn resequence(chunks: &mut [ParsedChunk]) {
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
    }
}

fn parse_epub_xhtml_text(
    text: &str,
    path: String,
    token_limit: i64,
) -> Result<Vec<ParsedChunk>, String> {
    let segments = html_segments(text);
    let groups = html_text_groups(text, &segments, token_limit);
    let mut chunks = Vec::new();
    let path = Some(path);

    for (index, (start, end)) in groups.into_iter().enumerate() {
        let raw = text
            .get(start..end)
            .ok_or_else(|| format!("Invalid EPUB XHTML segment range {start}:{end}"))?;
        let block_ref = epub_block_ref(&path, index, EPUB_TEXT_KIND, start, end);
        let (source_text, map_json) =
            protect_html(raw, DocumentFormat::Epub, ContentFormat::Xhtml, block_ref)?;
        chunks.push(ParsedChunk {
            sequence: index as i64,
            preprocessed_text: raw.to_string(),
            source_text,
            map_json,
        });
    }

    for segment in segments
        .iter()
        .filter(|segment| matches!(segment.kind, HtmlSegmentKind::Attribute))
    {
        let block_ref = epub_block_ref(
            &path,
            chunks.len(),
            EPUB_ATTRIBUTE_KIND,
            segment.start,
            segment.end,
        );
        let value = text
            .get(segment.start..segment.end)
            .ok_or_else(|| {
                format!(
                    "Invalid EPUB XHTML attribute range {}:{}",
                    segment.start, segment.end
                )
            })?
            .to_string();
        let map = super::types::PlaceholderMap::empty(
            DocumentFormat::Epub,
            ContentFormat::Xhtml,
            block_ref,
        );
        chunks.push(ParsedChunk {
            sequence: chunks.len() as i64,
            preprocessed_text: value.clone(),
            source_text: value,
            map_json: map.to_json()?,
        });
    }

    Ok(chunks)
}

fn render_epub_xhtml_entry(
    name: &str,
    original_text: &str,
    chunks: &[RenderedChunk],
) -> Result<String, String> {
    let mut patches = Vec::new();
    let mut legacy = Vec::new();
    let normalized_name = normalize_zip_path(name);

    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if !path_matches(
            map.block_ref.path.as_deref(),
            Some(normalized_name.as_str()),
        ) {
            continue;
        }

        match map.block_ref.kind.as_str() {
            EPUB_TEXT_KIND
            | EPUB_ATTRIBUTE_KIND
            | EPUB_LEGACY_HTML_TEXT_KIND
            | EPUB_LEGACY_HTML_ATTRIBUTE_KIND => {
                let Some(pointer) = map.block_ref.pointer.as_deref() else {
                    continue;
                };
                let Some((start, end)) = parse_range_pointer(pointer) else {
                    continue;
                };
                patches.push(HtmlPatch {
                    start,
                    end,
                    replacement: chunk.translated_text.clone(),
                });
            }
            EPUB_LEGACY_TEXT_BLOCK_KIND => {
                let order = map
                    .block_ref
                    .index
                    .map(|index| index as i64)
                    .unwrap_or(chunk.sequence);
                legacy.push((order, chunk.sequence, chunk.translated_text.as_str()));
            }
            _ => {}
        }
    }

    if !patches.is_empty() {
        apply_html_patches(original_text, &patches)
    } else if !legacy.is_empty() {
        legacy.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        Ok(legacy
            .into_iter()
            .map(|(_, _, text)| text)
            .collect::<String>())
    } else {
        Ok(original_text.to_string())
    }
}

fn epub_block_ref(
    path: &Option<String>,
    index: usize,
    kind: &str,
    start: usize,
    end: usize,
) -> BlockRef {
    BlockRef {
        kind: kind.into(),
        path: path.clone(),
        index: Some(index),
        pointer: Some(format!("range:{start}:{end}")),
        prefix: String::new(),
        suffix: String::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlSegmentKind {
    Text,
    Attribute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlSegment {
    kind: HtmlSegmentKind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlTag {
    name: String,
    start: usize,
    end: usize,
    closing: bool,
    self_closing: bool,
    raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlPatch {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlAttribute {
    name: String,
    value_start: usize,
    value_end: usize,
}

fn html_segments(text: &str) -> Vec<HtmlSegment> {
    let mut segments = Vec::new();
    let mut stack = Vec::<String>::new();
    let mut index = 0_usize;

    while index < text.len() {
        let Some(next_tag_offset) = text[index..].find('<') else {
            push_text_segment(text, index, text.len(), &stack, &mut segments);
            break;
        };
        let tag_start = index + next_tag_offset;
        push_text_segment(text, index, tag_start, &stack, &mut segments);

        let Some(tag) = parse_tag_at(text, tag_start) else {
            push_text_segment(text, tag_start, text.len(), &stack, &mut segments);
            break;
        };

        if !tag.closing {
            push_attribute_segments(&tag, &stack, &mut segments);
        }
        update_stack(&tag, &mut stack);
        index = if !tag.closing && matches!(tag.name.as_str(), "script" | "style") {
            let next_index = skip_raw_text_element(text, &tag).unwrap_or(tag.end);
            if stack.last().is_some_and(|name| name == &tag.name) {
                stack.pop();
            }
            next_index
        } else {
            tag.end
        };
    }

    segments
}

fn push_text_segment(
    text: &str,
    start: usize,
    end: usize,
    stack: &[String],
    segments: &mut Vec<HtmlSegment>,
) {
    if start >= end || !is_translatable_context(stack) {
        return;
    }
    let raw = &text[start..end];
    if raw.trim().is_empty() {
        return;
    }
    let core_start = start + leading_whitespace_len(raw);
    let core_end = end - trailing_whitespace_len(raw);
    if core_start < core_end {
        segments.push(HtmlSegment {
            kind: HtmlSegmentKind::Text,
            start: core_start,
            end: core_end,
        });
    }
}

fn push_attribute_segments(tag: &HtmlTag, stack: &[String], segments: &mut Vec<HtmlSegment>) {
    if !is_translatable_context_for_start_tag(stack, &tag.name) {
        return;
    }
    for attribute in parse_attributes(tag) {
        if !TRANSLATABLE_ATTRIBUTES
            .iter()
            .any(|name| attribute.name.eq_ignore_ascii_case(name))
        {
            continue;
        }
        if attribute.value_start >= attribute.value_end {
            continue;
        }
        let value = &tag.raw[attribute.value_start - tag.start..attribute.value_end - tag.start];
        if value.trim().is_empty() {
            continue;
        }
        let core_start = attribute.value_start + leading_whitespace_len(value);
        let core_end = attribute.value_end - trailing_whitespace_len(value);
        if core_start < core_end {
            segments.push(HtmlSegment {
                kind: HtmlSegmentKind::Attribute,
                start: core_start,
                end: core_end,
            });
        }
    }
}

fn parse_tag_at(text: &str, start: usize) -> Option<HtmlTag> {
    if text.get(start..)?.starts_with("<!--") {
        let end = start + text[start..].find("-->")? + 3;
        return Some(HtmlTag {
            name: String::new(),
            start,
            end,
            closing: false,
            self_closing: true,
            raw: text[start..end].to_string(),
        });
    }
    if text.get(start..)?.starts_with("<![CDATA[") {
        let end = start + text[start..].find("]]>")? + 3;
        return Some(HtmlTag {
            name: String::new(),
            start,
            end,
            closing: false,
            self_closing: true,
            raw: text[start..end].to_string(),
        });
    }
    if text.get(start..)?.starts_with("<!") || text.get(start..)?.starts_with("<?") {
        let end = find_tag_end(text, start)?;
        return Some(HtmlTag {
            name: String::new(),
            start,
            end,
            closing: false,
            self_closing: true,
            raw: text[start..end].to_string(),
        });
    }

    let end = find_tag_end(text, start)?;
    let raw = &text[start..end];
    let mut cursor = 1_usize;
    skip_ascii_whitespace(raw, &mut cursor);
    let closing = raw[cursor..].starts_with('/');
    if closing {
        cursor += 1;
        skip_ascii_whitespace(raw, &mut cursor);
    }
    let name_start = cursor;
    while cursor < raw.len() {
        let byte = raw.as_bytes()[cursor];
        if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
            break;
        }
        cursor += 1;
    }
    if name_start == cursor {
        return Some(HtmlTag {
            name: String::new(),
            start,
            end,
            closing,
            self_closing: true,
            raw: raw.to_string(),
        });
    }
    let name = raw[name_start..cursor].to_ascii_lowercase();
    let self_closing =
        raw[..raw.len().saturating_sub(1)].trim_end().ends_with('/') || is_void_element(&name);

    Some(HtmlTag {
        name,
        start,
        end,
        closing,
        self_closing,
        raw: raw.to_string(),
    })
}

fn find_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    let mut quote = None::<u8>;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(quote_byte) = quote {
            if byte == quote_byte {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn update_stack(tag: &HtmlTag, stack: &mut Vec<String>) {
    if tag.name.is_empty() {
        return;
    }
    if tag.closing {
        if let Some(position) = stack.iter().rposition(|name| name == &tag.name) {
            stack.truncate(position);
        }
    } else if !tag.self_closing {
        stack.push(tag.name.clone());
    }
}

fn skip_raw_text_element(text: &str, tag: &HtmlTag) -> Option<usize> {
    let close_pattern = format!("</{}", tag.name);
    let lower_tail = text.get(tag.end..)?.to_ascii_lowercase();
    let close_offset = lower_tail.find(&close_pattern)?;
    let close_start = tag.end + close_offset;
    parse_tag_at(text, close_start).map(|close_tag| close_tag.end)
}

fn is_translatable_context(stack: &[String]) -> bool {
    if stack
        .iter()
        .any(|name| matches!(name.as_str(), "script" | "style" | "link" | "meta"))
    {
        return false;
    }
    if stack.iter().any(|name| name == "head") && !stack.iter().any(|name| name == "title") {
        return false;
    }
    true
}

fn is_translatable_context_for_start_tag(stack: &[String], tag_name: &str) -> bool {
    if matches!(tag_name, "script" | "style" | "link" | "meta") {
        return false;
    }
    is_translatable_context(stack)
}

fn parse_attributes(tag: &HtmlTag) -> Vec<HtmlAttribute> {
    let raw = tag.raw.as_str();
    if tag.closing || raw.len() < 2 {
        return Vec::new();
    }

    let mut cursor = 1_usize;
    skip_ascii_whitespace(raw, &mut cursor);
    while cursor < raw.len() {
        let byte = raw.as_bytes()[cursor];
        if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
            break;
        }
        cursor += 1;
    }

    let mut attributes = Vec::new();
    while cursor < raw.len() {
        skip_ascii_whitespace(raw, &mut cursor);
        if cursor >= raw.len() || matches!(raw.as_bytes()[cursor], b'/' | b'>') {
            break;
        }

        let name_start = cursor;
        while cursor < raw.len() {
            let byte = raw.as_bytes()[cursor];
            if byte.is_ascii_whitespace() || matches!(byte, b'=' | b'/' | b'>') {
                break;
            }
            cursor += 1;
        }
        if name_start == cursor {
            cursor += 1;
            continue;
        }
        let name = raw[name_start..cursor].to_ascii_lowercase();
        skip_ascii_whitespace(raw, &mut cursor);
        if cursor >= raw.len() || raw.as_bytes()[cursor] != b'=' {
            continue;
        }
        cursor += 1;
        skip_ascii_whitespace(raw, &mut cursor);
        if cursor >= raw.len() {
            break;
        }

        let (value_start, value_end) = if matches!(raw.as_bytes()[cursor], b'\'' | b'"') {
            let quote = raw.as_bytes()[cursor];
            cursor += 1;
            let value_start = cursor;
            while cursor < raw.len() && raw.as_bytes()[cursor] != quote {
                cursor += 1;
            }
            let value_end = cursor;
            if cursor < raw.len() {
                cursor += 1;
            }
            (value_start, value_end)
        } else {
            let value_start = cursor;
            while cursor < raw.len() {
                let byte = raw.as_bytes()[cursor];
                if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
                    break;
                }
                cursor += 1;
            }
            (value_start, cursor)
        };

        attributes.push(HtmlAttribute {
            name,
            value_start: tag.start + value_start,
            value_end: tag.start + value_end,
        });
    }
    attributes
}

fn html_text_groups(text: &str, segments: &[HtmlSegment], token_limit: i64) -> Vec<(usize, usize)> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for segment in segments
        .iter()
        .filter(|segment| matches!(segment.kind, HtmlSegmentKind::Text))
    {
        let (start, end) = expand_inline_range(text, segment.start, segment.end);
        if let Some(last) = groups.last_mut() {
            if can_merge_text_ranges(text, last.1, start) {
                last.1 = end;
                continue;
            }
        }
        groups.push((start, end));
    }
    groups
        .into_iter()
        .flat_map(|(start, end)| split_text_range(text, start, end, token_limit))
        .collect()
}

fn expand_inline_range(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    loop {
        let Some(tag_start) = immediately_preceding_tag_start(text, start) else {
            break;
        };
        let Some(tag) = parse_tag_at(text, tag_start) else {
            break;
        };
        if tag.end != start || tag.closing || !is_placeholder_inline_tag(&tag.name) {
            break;
        }
        start = tag.start;
    }

    loop {
        if !text.get(end..).is_some_and(|rest| rest.starts_with("</")) {
            break;
        }
        let Some(tag) = parse_tag_at(text, end) else {
            break;
        };
        if !tag.closing || !is_placeholder_inline_tag(&tag.name) {
            break;
        }
        end = tag.end;
    }

    (start, end)
}

fn immediately_preceding_tag_start(text: &str, end: usize) -> Option<usize> {
    let before = text.get(..end)?;
    let tag_start = before.rfind('<')?;
    let tag_end = before.rfind('>');
    if tag_end.is_some_and(|tag_end| tag_end > tag_start) {
        return None;
    }
    Some(tag_start)
}

fn can_merge_text_ranges(text: &str, previous_end: usize, next_start: usize) -> bool {
    if previous_end > next_start {
        return false;
    }
    let mut cursor = previous_end;
    while cursor < next_start {
        let rest = &text[cursor..next_start];
        if rest.starts_with(char::is_whitespace) {
            let character = rest.chars().next().expect("non-empty rest");
            cursor += character.len_utf8();
            continue;
        }
        if !rest.starts_with('<') {
            return false;
        }
        let Some(tag) = parse_tag_at(text, cursor) else {
            return false;
        };
        if tag.end > next_start || tag.self_closing || !is_placeholder_inline_tag(&tag.name) {
            return false;
        }
        cursor = tag.end;
    }
    true
}

fn split_text_range(text: &str, start: usize, end: usize, token_limit: i64) -> Vec<(usize, usize)> {
    let Some(raw) = text.get(start..end) else {
        return Vec::new();
    };
    if raw.contains('<') {
        return vec![(start, end)];
    }
    let chunked = chunk_raw_block_refs(
        vec![RawBlockRef::new(raw.to_string(), true, ())],
        token_limit_usize(token_limit),
    );
    if chunked.len() <= 1 {
        return vec![(start, end)];
    }
    let mut ranges = Vec::new();
    for chunk in chunked {
        if chunk.is_empty() {
            continue;
        }
        let chunk_start = chunk
            .first()
            .map(|block| start + block.source_start)
            .unwrap_or(start);
        let chunk_end = chunk
            .last()
            .map(|block| start + block.source_end)
            .unwrap_or(end);
        if chunk_start < chunk_end {
            ranges.push((chunk_start, chunk_end));
        }
    }
    ranges
}

fn is_placeholder_inline_tag(name: &str) -> bool {
    matches!(
        name,
        "a" | "abbr"
            | "b"
            | "cite"
            | "code"
            | "data"
            | "dfn"
            | "em"
            | "i"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
    )
}

fn apply_html_patches(text: &str, patches: &[HtmlPatch]) -> Result<String, String> {
    let mut patches = patches.to_vec();
    patches.sort_by(|left, right| left.start.cmp(&right.start).then(left.end.cmp(&right.end)));
    let mut occupied = HashSet::new();
    let mut previous_end = 0_usize;
    for patch in &patches {
        if patch.start >= patch.end || patch.end > text.len() {
            return Err(format!(
                "Invalid EPUB XHTML replacement range {}:{}",
                patch.start, patch.end
            ));
        }
        if !text.is_char_boundary(patch.start) || !text.is_char_boundary(patch.end) {
            return Err(format!(
                "EPUB XHTML replacement range is not on UTF-8 boundaries {}:{}",
                patch.start, patch.end
            ));
        }
        if patch.start < previous_end {
            return Err("Overlapping EPUB XHTML replacement ranges".into());
        }
        if !occupied.insert((patch.start, patch.end)) {
            return Err("Duplicate EPUB XHTML replacement range".into());
        }
        previous_end = patch.end;
    }

    let mut rendered = text.to_string();
    for patch in patches.iter().rev() {
        rendered.replace_range(patch.start..patch.end, &patch.replacement);
    }
    Ok(rendered)
}

fn parse_range_pointer(pointer: &str) -> Option<(usize, usize)> {
    let rest = pointer.strip_prefix("range:")?;
    let (start, end) = rest.split_once(':')?;
    let start = start.parse::<usize>().ok()?;
    let end = end.parse::<usize>().ok()?;
    if start < end {
        Some((start, end))
    } else {
        None
    }
}

fn path_matches(map_path: Option<&str>, target_path: Option<&str>) -> bool {
    match target_path {
        Some(target) => map_path
            .map(normalize_zip_path)
            .is_some_and(|path| path == target),
        None => map_path.is_none(),
    }
}

fn normalize_zip_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn leading_whitespace_len(text: &str) -> usize {
    text.char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(text.len())
}

fn trailing_whitespace_len(text: &str) -> usize {
    text.char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!character.is_whitespace()).then_some(text.len() - (index + character.len_utf8()))
        })
        .unwrap_or(text.len())
}

fn skip_ascii_whitespace(text: &str, cursor: &mut usize) {
    while *cursor < text.len() && text.as_bytes()[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::document_parsing::types::{BlockRef, PlaceholderMap};

    use super::*;

    #[test]
    fn render_document_writes_all_chunks_for_epub_html_entry_in_order() {
        let path = unique_temp_epub_path();
        write_test_epub(&path, "OEBPS/chapter.xhtml", "original").expect("write epub");
        let chunks = vec![
            rendered_chunk("OEBPS/chapter.xhtml", Some(1), 20, "second"),
            rendered_chunk("OEBPS\\chapter.xhtml", Some(0), 10, "first"),
        ];

        let output = EpubParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &chunks,
            })
            .expect("render epub");
        let rendered = read_epub_entry(output, "OEBPS/chapter.xhtml").expect("read rendered entry");

        assert_eq!(rendered, "firstsecond");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn render_document_patches_epub_xhtml_ranges_and_keeps_other_entries() {
        let path = unique_temp_epub_path();
        write_test_epub_with_entries(
            &path,
            &[
                ("OEBPS/chapter.xhtml", "<p>Hello <strong>World</strong></p>"),
                ("OEBPS/image.txt", "unchanged"),
            ],
        )
        .expect("write epub");
        let parsed = parse_epub_xhtml_text(
            "<p>Hello <strong>World</strong></p>",
            "OEBPS/chapter.xhtml".into(),
            800,
        )
        .expect("parse xhtml");
        let chunks = parsed
            .iter()
            .map(|chunk| RenderedChunk {
                sequence: chunk.sequence,
                source_text: chunk.source_text.clone(),
                after_translate_text: "Hola <t1>Mundo</t1>".into(),
                translated_text: super::super::placeholders::restore_from_json(
                    &chunk.map_json,
                    "Hola <t1>Mundo</t1>",
                )
                .expect("restore html"),
                map_json: chunk.map_json.clone(),
            })
            .collect::<Vec<_>>();

        let output = EpubParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &chunks,
            })
            .expect("render epub");
        let rendered =
            read_epub_entry(output.clone(), "OEBPS/chapter.xhtml").expect("read rendered xhtml");
        let other = read_epub_entry(output, "OEBPS/image.txt").expect("read other entry");

        assert_eq!(rendered, "<p>Hola <strong>Mundo</strong></p>");
        assert_eq!(other, "unchanged");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn render_document_keeps_xhtml_self_closing_tags_out_of_scraper_dom() {
        let path = unique_temp_epub_path();
        let xhtml = r#"<p>Alpha<span />Beta</p>"#;
        write_test_epub(&path, "OEBPS/chapter.xhtml", xhtml).expect("write epub");
        let parsed =
            parse_epub_xhtml_text(xhtml, "OEBPS/chapter.xhtml".into(), 800).expect("parse xhtml");
        let chunks = parsed
            .iter()
            .map(|chunk| {
                let translated = chunk
                    .source_text
                    .replace("Alpha", "Uno")
                    .replace("Beta", "Dos");
                RenderedChunk {
                    sequence: chunk.sequence,
                    source_text: chunk.source_text.clone(),
                    after_translate_text: translated.clone(),
                    translated_text: super::super::placeholders::restore_from_json(
                        &chunk.map_json,
                        &translated,
                    )
                    .expect("restore xhtml"),
                    map_json: chunk.map_json.clone(),
                }
            })
            .collect::<Vec<_>>();

        let output = EpubParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &chunks,
            })
            .expect("render epub");
        let rendered = read_epub_entry(output, "OEBPS/chapter.xhtml").expect("read rendered xhtml");

        assert_eq!(rendered, r#"<p>Uno<span />Dos</p>"#);
        let _ = std::fs::remove_file(path);
    }

    fn rendered_chunk(
        path: &str,
        index: Option<usize>,
        sequence: i64,
        translated_text: &str,
    ) -> RenderedChunk {
        let map = PlaceholderMap::empty(
            DocumentFormat::Epub,
            ContentFormat::Xhtml,
            BlockRef {
                kind: "text-block".into(),
                path: Some(path.into()),
                index,
                pointer: None,
                prefix: String::new(),
                suffix: String::new(),
            },
        );
        RenderedChunk {
            sequence,
            source_text: String::new(),
            after_translate_text: translated_text.into(),
            translated_text: translated_text.into(),
            map_json: map.to_json().expect("serialize map"),
        }
    }

    fn write_test_epub(path: &std::path::Path, entry: &str, text: &str) -> Result<(), String> {
        write_test_epub_with_entries(path, &[(entry, text)])
    }

    fn write_test_epub_with_entries(
        path: &std::path::Path,
        entries: &[(&str, &str)],
    ) -> Result<(), String> {
        let file = File::create(path).map_err(|error| error.to_string())?;
        let mut writer = ZipWriter::new(file);
        for (entry, text) in entries {
            writer
                .start_file(entry, SimpleFileOptions::default())
                .map_err(|error| error.to_string())?;
            writer
                .write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        writer.finish().map_err(|error| error.to_string())?;
        Ok(())
    }

    fn read_epub_entry(bytes: Vec<u8>, entry: &str) -> Result<String, String> {
        let reader = Cursor::new(bytes);
        let mut archive = ZipArchive::new(reader).map_err(|error| error.to_string())?;
        let mut file = archive.by_name(entry).map_err(|error| error.to_string())?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        Ok(text)
    }

    fn unique_temp_epub_path() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "insitu-epub-render-{}-{nanos}.epub",
            std::process::id()
        ))
    }
}
