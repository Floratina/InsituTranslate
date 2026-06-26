use std::collections::HashMap;

use lrc::{Lyrics, TimeTag};
use regex::Regex;
use subparse::{parse_str, SubtitleFormat};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::placeholders::{protect_html, restore_with_map};
use super::types::{
    BlockRef, ParsedChunk, PlaceholderEntry, PlaceholderMap, RenderInput, RenderedChunk,
};
use super::{
    chunk_raw_block_refs, token_limit_usize, ChunkedRawBlock, DocumentParser, RawBlockRef,
};

const TIMED_TEXT_CHUNK_KIND: &str = "timed-text-chunk";
const TIMED_UNIT_KIND: &str = "timed-unit";
const LEGACY_TIMED_TEXT_KIND: &str = "timed-text";
const ASS_CONTROL_KIND: &str = "ass-control";
const SUBPARSE_FPS: f64 = 25.0;

pub struct SubtitleParser {
    pub format: DocumentFormat,
    pub content_format: ContentFormat,
}

impl DocumentParser for SubtitleParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read subtitle source: {error}"))?;
        match self.format {
            DocumentFormat::Srt | DocumentFormat::Ass => {
                parse_subtitle_text(&text, self.format, self.content_format, input.token_limit)
            }
            DocumentFormat::Lrc => parse_lrc_text(&text, input.token_limit),
            _ => Err("Unsupported subtitle parser format".into()),
        }
    }

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        let map = super::parse_map(map_json)?;
        if map.block_ref.kind == TIMED_TEXT_CHUNK_KIND {
            restore_timed_text_chunk(&map, after_translate_text)
        } else {
            super::placeholders::restore_from_json(map_json, after_translate_text)
        }
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read timed text source for render: {error}"))?;
        match self.format {
            DocumentFormat::Srt => render_srt_document(&text, input.chunks).map(String::into_bytes),
            DocumentFormat::Ass => render_ass_document(&text, input.chunks),
            DocumentFormat::Lrc => render_lrc_document(&text, input.chunks).map(String::into_bytes),
            _ => Err("Unsupported subtitle parser format".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRange {
    index: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimedTextUnit {
    target_ref: TimedTargetRef,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimedTargetKind {
    Entry,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimedTargetRef {
    kind: TimedTargetKind,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtectedTimedUnit {
    target_ref: TimedTargetRef,
    original_text: String,
    protected_text: String,
    placeholder_entries: Vec<PlaceholderEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimedUnitDescriptor {
    tag: String,
    target_ref: TimedTargetRef,
}

fn parse_subtitle_text(
    text: &str,
    format: DocumentFormat,
    content_format: ContentFormat,
    token_limit: i64,
) -> Result<Vec<ParsedChunk>, String> {
    let subtitle_format = subtitle_format(format)?;
    let subtitle_file = parse_str(subtitle_format, text, SUBPARSE_FPS)
        .map_err(|error| format!("Unable to parse subtitle source with subparse: {error}"))?;
    let entries = subtitle_file
        .get_subtitle_entries()
        .map_err(|error| format!("Unable to read subtitle entries: {error}"))?;
    let srt_ranges = if format == DocumentFormat::Srt {
        let ranges = srt_body_ranges(text)?;
        if ranges.len() != entries.len() {
            return Err(format!(
                "SRT source range mismatch: found {} body ranges but subparse found {} entries",
                ranges.len(),
                entries.len()
            ));
        }
        Some(ranges)
    } else {
        None
    };

    let mut units = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let source_text = if let Some(ranges) = srt_ranges.as_ref() {
            text.get(ranges[index].start..ranges[index].end)
                .ok_or_else(|| format!("Invalid SRT body range for entry {index}"))?
                .to_string()
        } else {
            entry.line.unwrap_or_default()
        };
        if source_text.trim().is_empty() {
            continue;
        }
        units.push(TimedTextUnit {
            target_ref: TimedTargetRef {
                kind: TimedTargetKind::Entry,
                index,
            },
            text: source_text,
        });
    }

    build_timed_chunks(units, format, content_format, token_limit)
}

fn parse_lrc_text(text: &str, token_limit: i64) -> Result<Vec<ParsedChunk>, String> {
    Lyrics::from_str(text)
        .map_err(|error| format!("Unable to parse LRC source with lrc: {error}"))?;
    let units = lrc_lyric_ranges(text)?
        .into_iter()
        .filter_map(|(line_index, range)| {
            let text = text.get(range.start..range.end)?.to_string();
            if text.trim().is_empty() {
                return None;
            }
            Some(TimedTextUnit {
                target_ref: TimedTargetRef {
                    kind: TimedTargetKind::Line,
                    index: line_index,
                },
                text,
            })
        })
        .collect::<Vec<_>>();

    build_timed_chunks(units, DocumentFormat::Lrc, ContentFormat::Lrc, token_limit)
}

fn build_timed_chunks(
    units: Vec<TimedTextUnit>,
    format: DocumentFormat,
    content_format: ContentFormat,
    token_limit: i64,
) -> Result<Vec<ParsedChunk>, String> {
    let raw_blocks = units
        .into_iter()
        .map(|unit| {
            RawBlockRef::new(
                unit.text.clone(),
                subtitle_unit_is_breakable(format, &unit.text),
                unit,
            )
        })
        .collect::<Vec<_>>();
    chunk_raw_block_refs(raw_blocks, token_limit_usize(token_limit))
        .into_iter()
        .enumerate()
        .map(|(sequence, units)| timed_chunk_from_units(sequence, units, format, content_format))
        .collect()
}

fn protect_timed_unit(
    unit: &ChunkedRawBlock<TimedTextUnit>,
    format: DocumentFormat,
    content_format: ContentFormat,
    next_placeholder_index: &mut usize,
) -> Result<ProtectedTimedUnit, String> {
    let unit_block_ref = BlockRef {
        kind: TIMED_UNIT_KIND.into(),
        path: None,
        index: Some(unit.metadata.target_ref.index),
        pointer: Some(unit_target_ref(&unit.metadata.target_ref)),
        prefix: String::new(),
        suffix: String::new(),
    };

    let (protected_text, placeholder_entries) = match format {
        DocumentFormat::Srt => {
            let (source, map_json) =
                protect_html(&unit.text, format, content_format, unit_block_ref)?;
            let map = super::parse_map(&map_json)?;
            renumber_placeholder_entries(source, map.entries, next_placeholder_index, "")
        }
        DocumentFormat::Ass => protect_ass_text(&unit.text, next_placeholder_index),
        _ => (unit.text.clone(), Vec::new()),
    };

    Ok(ProtectedTimedUnit {
        target_ref: unit.metadata.target_ref,
        original_text: unit.text.clone(),
        protected_text,
        placeholder_entries,
    })
}

fn timed_chunk_from_units(
    sequence: usize,
    units: Vec<ChunkedRawBlock<TimedTextUnit>>,
    format: DocumentFormat,
    content_format: ContentFormat,
) -> Result<ParsedChunk, String> {
    let mut source_parts = Vec::new();
    let mut preprocessed_parts = Vec::new();
    let mut entries = Vec::new();
    let unit_count = units.len();
    let mut next_placeholder_index = 1_usize;

    for (local_index, unit) in units.iter().enumerate() {
        let mut protected =
            protect_timed_unit(unit, format, content_format, &mut next_placeholder_index)?;
        let unit_tag = format!("it{local_index}");
        source_parts.push(format!(
            "<{unit_tag}>{}</{unit_tag}>",
            protected.protected_text
        ));
        preprocessed_parts.push(format!(
            "<{unit_tag}>{}</{unit_tag}>",
            protected.original_text
        ));
        entries.push(PlaceholderEntry {
            id: unit_tag.clone(),
            kind: TIMED_UNIT_KIND.into(),
            original: protected.original_text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(unit_target_ref(&protected.target_ref)),
        });
        for entry in &mut protected.placeholder_entries {
            entry.native_ref = Some(unit_placeholder_ref(&unit_tag, entry.native_ref.take()));
        }
        entries.extend(protected.placeholder_entries);
    }

    let map = PlaceholderMap {
        version: super::types::PLACEHOLDER_MAP_VERSION,
        format,
        content_format,
        block_ref: BlockRef {
            kind: TIMED_TEXT_CHUNK_KIND.into(),
            path: None,
            index: Some(sequence),
            pointer: Some(format!("units:{unit_count}")),
            prefix: String::new(),
            suffix: String::new(),
        },
        entries,
    };

    Ok(ParsedChunk {
        sequence: sequence as i64,
        preprocessed_text: preprocessed_parts.join("\n"),
        source_text: source_parts.join("\n"),
        map_json: map.to_json()?,
    })
}

fn subtitle_unit_is_breakable(format: DocumentFormat, text: &str) -> bool {
    match format {
        DocumentFormat::Srt => !text.contains('<') && !text.contains('>'),
        DocumentFormat::Ass => ass_control_ranges(text).is_empty(),
        DocumentFormat::Lrc => !text.contains('[') && !text.contains(']'),
        _ => true,
    }
}

fn protect_ass_text(
    text: &str,
    next_placeholder_index: &mut usize,
) -> (String, Vec<PlaceholderEntry>) {
    let ranges = ass_control_ranges(text);
    if ranges.is_empty() {
        return (text.to_string(), Vec::new());
    }

    let mut output = String::with_capacity(text.len());
    let mut entries = Vec::new();
    let mut last_end = 0_usize;

    for range in ranges {
        output.push_str(&text[last_end..range.start]);
        let id = format!("t{}", *next_placeholder_index);
        *next_placeholder_index += 1;
        let original = text[range.start..range.end].to_string();
        output.push_str(&format!("<{id}></{id}>"));
        entries.push(PlaceholderEntry {
            id,
            kind: ASS_CONTROL_KIND.into(),
            original: String::new(),
            open: original,
            close: String::new(),
            translatable: false,
            native_ref: None,
        });
        last_end = range.end;
    }
    output.push_str(&text[last_end..]);

    (output, entries)
}

fn ass_control_ranges(text: &str) -> Vec<TextRange> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0_usize;

    while index < bytes.len() {
        if bytes[index] == b'{' {
            if let Some(close_offset) = text[index + 1..].find('}') {
                let end = index + 1 + close_offset + 1;
                ranges.push(TextRange { start: index, end });
                index = end;
                continue;
            }
        }
        if bytes[index] == b'\\'
            && index + 1 < bytes.len()
            && matches!(bytes[index + 1], b'N' | b'n' | b'h')
        {
            ranges.push(TextRange {
                start: index,
                end: index + 2,
            });
            index += 2;
            continue;
        }
        index += 1;
    }

    ranges
}

fn renumber_placeholder_entries(
    mut source: String,
    mut entries: Vec<PlaceholderEntry>,
    next_placeholder_index: &mut usize,
    unit_tag: &str,
) -> (String, Vec<PlaceholderEntry>) {
    for entry in &mut entries {
        let old_id = entry.id.clone();
        let new_id = format!("t{}", *next_placeholder_index);
        *next_placeholder_index += 1;
        source = source
            .replace(&format!("<{old_id}>"), &format!("<{new_id}>"))
            .replace(&format!("</{old_id}>"), &format!("</{new_id}>"));
        entry.id = new_id;
        if !unit_tag.is_empty() {
            entry.native_ref = Some(unit_placeholder_ref(unit_tag, entry.native_ref.take()));
        }
    }
    (source, entries)
}

fn restore_timed_text_chunk(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    let units = timed_unit_descriptors(map)?;
    let translated_units = extract_timed_unit_text(after_translate_text, &units)?;
    let mut restored_units = Vec::new();

    for (unit, translated_text) in units.iter().zip(translated_units) {
        let mut unit_map = map.clone();
        unit_map.block_ref = BlockRef {
            kind: TIMED_UNIT_KIND.into(),
            path: None,
            index: Some(unit.target_ref.index),
            pointer: Some(unit_target_ref(&unit.target_ref)),
            prefix: String::new(),
            suffix: String::new(),
        };
        unit_map.entries = map
            .entries
            .iter()
            .filter(|entry| placeholder_belongs_to_unit(entry, &unit.tag))
            .cloned()
            .collect();
        let restored = if unit_map.entries.is_empty() {
            translated_text
        } else {
            restore_with_map(&unit_map, &translated_text)?
        };
        restored_units.push(format!("<{}>{}</{}>", unit.tag, restored, unit.tag));
    }

    Ok(restored_units.join("\n"))
}

fn render_srt_document(text: &str, chunks: &[RenderedChunk]) -> Result<String, String> {
    let subtitle_file = parse_str(SubtitleFormat::SubRip, text, SUBPARSE_FPS)
        .map_err(|error| format!("Unable to validate SRT source with subparse: {error}"))?;
    let entries = subtitle_file
        .get_subtitle_entries()
        .map_err(|error| format!("Unable to read SRT entries: {error}"))?;
    let ranges = srt_body_ranges(text)?;
    if ranges.len() != entries.len() {
        return Err(format!(
            "SRT source range mismatch: found {} body ranges but subparse found {} entries",
            ranges.len(),
            entries.len()
        ));
    }

    let Some(translations) = collect_timed_translations(chunks, TimedTargetKind::Entry)? else {
        return legacy_or_original(text, chunks);
    };
    let mut patches = Vec::new();
    for (entry_index, replacement) in translations {
        let Some(range) = ranges.get(entry_index).copied() else {
            return Err(format!(
                "Translated SRT entry index {entry_index} does not exist in source"
            ));
        };
        patches.push(TextPatch { range, replacement });
    }
    apply_text_patches(text, patches)
}

fn render_ass_document(text: &str, chunks: &[RenderedChunk]) -> Result<Vec<u8>, String> {
    let mut subtitle_file = parse_str(SubtitleFormat::SubStationAlpha, text, SUBPARSE_FPS)
        .map_err(|error| format!("Unable to parse ASS source with subparse: {error}"))?;
    let mut entries = subtitle_file
        .get_subtitle_entries()
        .map_err(|error| format!("Unable to read ASS entries: {error}"))?;
    let Some(translations) = collect_timed_translations(chunks, TimedTargetKind::Entry)? else {
        return Ok(legacy_or_original(text, chunks)?.into_bytes());
    };
    for (entry_index, replacement) in translations {
        let Some(entry) = entries.get_mut(entry_index) else {
            return Err(format!(
                "Translated ASS entry index {entry_index} does not exist in source"
            ));
        };
        entry.line = Some(replacement);
    }
    subtitle_file
        .update_subtitle_entries(&entries)
        .map_err(|error| format!("Unable to update ASS entries with subparse: {error}"))?;
    subtitle_file
        .to_data()
        .map_err(|error| format!("Unable to export ASS with subparse: {error}"))
}

fn render_lrc_document(text: &str, chunks: &[RenderedChunk]) -> Result<String, String> {
    Lyrics::from_str(text)
        .map_err(|error| format!("Unable to validate LRC source with lrc: {error}"))?;
    let line_ranges = lrc_lyric_ranges(text)?
        .into_iter()
        .collect::<HashMap<usize, TextRange>>();
    let Some(translations) = collect_timed_translations(chunks, TimedTargetKind::Line)? else {
        return legacy_or_original(text, chunks);
    };

    let mut patches = Vec::new();
    for (line_index, replacement) in translations {
        let Some(range) = line_ranges.get(&line_index).copied() else {
            return Err(format!(
                "Translated LRC line index {line_index} does not exist in source"
            ));
        };
        patches.push(TextPatch { range, replacement });
    }
    apply_text_patches(text, patches)
}

fn collect_timed_translations(
    chunks: &[RenderedChunk],
    expected_kind: TimedTargetKind,
) -> Result<Option<HashMap<usize, String>>, String> {
    let mut translations = HashMap::<usize, String>::new();
    let mut found_timed_chunks = false;

    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind != TIMED_TEXT_CHUNK_KIND {
            continue;
        }
        found_timed_chunks = true;
        let units = timed_unit_descriptors(&map)?;
        let tagged_text = if chunk.translated_text.contains("<it") {
            chunk.translated_text.clone()
        } else {
            restore_timed_text_chunk(&map, &chunk.after_translate_text)?
        };
        let translated_units = extract_timed_unit_text(&tagged_text, &units)?;
        for (unit, translated_text) in units.into_iter().zip(translated_units) {
            if unit.target_ref.kind != expected_kind {
                return Err("Timed text target kind does not match source format".into());
            }
            translations
                .entry(unit.target_ref.index)
                .and_modify(|existing| existing.push_str(&translated_text))
                .or_insert(translated_text);
        }
    }

    Ok(found_timed_chunks.then_some(translations))
}

fn timed_unit_descriptors(map: &PlaceholderMap) -> Result<Vec<TimedUnitDescriptor>, String> {
    let mut units = Vec::new();
    for entry in &map.entries {
        if entry.kind != TIMED_UNIT_KIND {
            continue;
        }
        let native_ref = entry
            .native_ref
            .as_deref()
            .ok_or_else(|| format!("Timed unit {} is missing native reference", entry.id))?;
        units.push(TimedUnitDescriptor {
            tag: entry.id.clone(),
            target_ref: parse_unit_target_ref(native_ref)?,
        });
    }
    if units.is_empty() {
        return Err("Timed text chunk has no timed unit entries".into());
    }
    Ok(units)
}

fn extract_timed_unit_text(
    text: &str,
    units: &[TimedUnitDescriptor],
) -> Result<Vec<String>, String> {
    let mut cursor = 0_usize;
    let mut values = Vec::new();

    for unit in units {
        let pattern = Regex::new(&format!(
            r"(?is)<\s*{}\s*>(.*?)<\s*/\s*{}\s*>",
            regex::escape(&unit.tag),
            regex::escape(&unit.tag)
        ))
        .map_err(|error| error.to_string())?;
        let Some(captures) = pattern.captures(&text[cursor..]) else {
            return Err(format!(
                "Translated timed text is missing expected unit tag <{}>",
                unit.tag
            ));
        };
        let full = captures
            .get(0)
            .ok_or_else(|| format!("Unable to read translated unit {}", unit.tag))?;
        if !text[cursor..cursor + full.start()].trim().is_empty() {
            return Err(format!(
                "Translated timed text contains unexpected content before <{}>",
                unit.tag
            ));
        }
        let value = captures
            .get(1)
            .map(|capture| capture.as_str().to_string())
            .unwrap_or_default();
        cursor += full.end();
        values.push(value);
    }

    if !text[cursor..].trim().is_empty() {
        return Err("Translated timed text contains unexpected trailing content".into());
    }

    Ok(values)
}

fn placeholder_belongs_to_unit(entry: &PlaceholderEntry, unit_tag: &str) -> bool {
    let Some(native_ref) = entry.native_ref.as_deref() else {
        return false;
    };
    native_ref == format!("unit:{unit_tag}") || native_ref.starts_with(&format!("unit:{unit_tag};"))
}

fn subtitle_format(format: DocumentFormat) -> Result<SubtitleFormat, String> {
    match format {
        DocumentFormat::Srt => Ok(SubtitleFormat::SubRip),
        DocumentFormat::Ass => Ok(SubtitleFormat::SubStationAlpha),
        _ => Err("Unsupported subparse subtitle format".into()),
    }
}

fn unit_target_ref(target_ref: &TimedTargetRef) -> String {
    match target_ref.kind {
        TimedTargetKind::Entry => format!("entry:{}", target_ref.index),
        TimedTargetKind::Line => format!("line:{}", target_ref.index),
    }
}

fn parse_unit_target_ref(value: &str) -> Result<TimedTargetRef, String> {
    if let Some(index) = value
        .strip_prefix("entry:")
        .and_then(|index| index.parse::<usize>().ok())
    {
        return Ok(TimedTargetRef {
            kind: TimedTargetKind::Entry,
            index,
        });
    }
    if let Some(index) = value
        .strip_prefix("line:")
        .and_then(|index| index.parse::<usize>().ok())
    {
        return Ok(TimedTargetRef {
            kind: TimedTargetKind::Line,
            index,
        });
    }
    Err(format!("Invalid timed unit native reference: {value}"))
}

fn unit_placeholder_ref(unit_tag: &str, native_ref: Option<String>) -> String {
    match native_ref {
        Some(value) if !value.is_empty() => format!("unit:{unit_tag};{value}"),
        _ => format!("unit:{unit_tag}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextPatch {
    range: TextRange,
    replacement: String,
}

fn apply_text_patches(text: &str, mut patches: Vec<TextPatch>) -> Result<String, String> {
    patches.sort_by(|left, right| right.range.start.cmp(&left.range.start));
    let mut previous_start = text.len();
    let mut output = text.to_string();

    for patch in patches {
        if patch.range.start > patch.range.end
            || patch.range.end > previous_start
            || !output.is_char_boundary(patch.range.start)
            || !output.is_char_boundary(patch.range.end)
        {
            return Err(format!(
                "Invalid timed text replacement range {}:{}",
                patch.range.start, patch.range.end
            ));
        }
        output.replace_range(patch.range.start..patch.range.end, &patch.replacement);
        previous_start = patch.range.start;
    }

    Ok(output)
}

fn legacy_or_original(text: &str, chunks: &[RenderedChunk]) -> Result<String, String> {
    let mut legacy = Vec::new();
    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind == LEGACY_TIMED_TEXT_KIND {
            let order = map
                .block_ref
                .index
                .map(|index| index as i64)
                .unwrap_or(chunk.sequence);
            legacy.push((order, chunk.sequence, chunk.translated_text.as_str()));
        }
    }

    if legacy.is_empty() {
        return Ok(text.to_string());
    }

    legacy.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    Ok(legacy
        .into_iter()
        .map(|(_, _, translated_text)| translated_text)
        .collect())
}

fn line_ranges(text: &str) -> Vec<LineRange> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0_usize;
    let mut line_index = 0_usize;

    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let mut end = index;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        ranges.push(LineRange {
            index: line_index,
            start,
            end,
        });
        line_index += 1;
        start = index + 1;
    }

    if start < text.len() {
        ranges.push(LineRange {
            index: line_index,
            start,
            end: text.len(),
        });
    }

    ranges
}

fn srt_body_ranges(text: &str) -> Result<Vec<TextRange>, String> {
    let lines = line_ranges(text);
    let mut ranges = Vec::new();
    let mut cursor = 0_usize;

    while cursor < lines.len() {
        while cursor < lines.len() && line_text(text, lines[cursor]).trim().is_empty() {
            cursor += 1;
        }
        if cursor >= lines.len() {
            break;
        }

        cursor += 1;
        if cursor >= lines.len() {
            return Err("Invalid SRT block: missing timestamp line".into());
        }
        if !line_text(text, lines[cursor]).contains("-->") {
            return Err(format!(
                "Invalid SRT block: expected timestamp line at source line {}",
                lines[cursor].index + 1
            ));
        }

        cursor += 1;
        let body_start_line = cursor;
        while cursor < lines.len() && !line_text(text, lines[cursor]).trim().is_empty() {
            cursor += 1;
        }
        if body_start_line == cursor {
            ranges.push(TextRange {
                start: lines
                    .get(body_start_line)
                    .map(|line| line.start)
                    .unwrap_or(text.len()),
                end: lines
                    .get(body_start_line)
                    .map(|line| line.start)
                    .unwrap_or(text.len()),
            });
        } else {
            ranges.push(TextRange {
                start: lines[body_start_line].start,
                end: lines[cursor - 1].end,
            });
        }
    }

    Ok(ranges)
}

fn lrc_lyric_ranges(text: &str) -> Result<Vec<(usize, TextRange)>, String> {
    let mut ranges = Vec::new();
    for line in line_ranges(text) {
        let raw = line_text(text, line);
        if raw.trim().is_empty() {
            continue;
        }
        let leading_ws = leading_whitespace_len(raw);
        let mut cursor = leading_ws;
        let mut has_time_tag = false;
        let mut has_id_tag = false;
        let mut had_tag = false;

        while cursor < raw.len() && raw[cursor..].starts_with('[') {
            let Some(close_offset) = raw[cursor..].find(']') else {
                break;
            };
            let tag_end = cursor + close_offset + 1;
            let tag = &raw[cursor..tag_end];
            if TimeTag::from_str(tag).is_ok() {
                has_time_tag = true;
                had_tag = true;
                cursor = tag_end;
            } else if tag.contains(':') {
                has_id_tag = true;
                had_tag = true;
                cursor = tag_end;
            } else {
                break;
            }
        }

        if has_time_tag {
            let lyric_start = cursor + leading_whitespace_len(&raw[cursor..]);
            if lyric_start <= raw.len() {
                ranges.push((
                    line.index,
                    TextRange {
                        start: line.start + lyric_start,
                        end: line.end,
                    },
                ));
            }
        } else if had_tag && has_id_tag {
            continue;
        } else {
            ranges.push((
                line.index,
                TextRange {
                    start: line.start,
                    end: line.end,
                },
            ));
        }
    }

    Ok(ranges)
}

fn line_text(text: &str, line: LineRange) -> &str {
    &text[line.start..line.end]
}

fn leading_whitespace_len(text: &str) -> usize {
    text.char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parsing::types::RenderedChunk;

    #[test]
    fn srt_chunks_adjacent_entries_without_splitting_single_subtitle() {
        let srt = concat!(
            "42\n",
            "00:00:01,000 --> 00:00:02,000\n",
            "Alpha\n\n",
            "99\n",
            "00:00:03,000 --> 00:00:04,000\n",
            "Beta\n\n",
            "100\n",
            "00:00:05,000 --> 00:00:06,000\n",
            "Gamma\n\n",
        );

        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 2)
            .expect("parse srt");

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].source_text.contains("<it0>Alpha</it0>"));
        assert!(chunks[0].source_text.contains("<it1>Beta</it1>"));
        assert!(chunks[1].source_text.contains("<it0>Gamma</it0>"));
    }

    #[test]
    fn srt_render_patches_only_body_and_preserves_crlf_timing_and_indices() {
        let srt = concat!(
            "42\r\n",
            "00:00:01,000 --> 00:00:02,000\r\n",
            "Hello <i>world</i>\r\n\r\n",
            "99\r\n",
            "00:00:03,000 --> 00:00:04,000\r\n",
            "Second line\r\n\r\n",
        );
        let chunks =
            parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100).expect("parse");
        let after = "<it0>Hola <t1>mundo</t1></it0>\n<it1>Segunda linea</it1>";
        let rendered_chunk = rendered_chunk(&chunks[0], after);

        let rendered = render_srt_document(srt, &[rendered_chunk]).expect("render srt");

        assert!(rendered.contains("42\r\n00:00:01,000 --> 00:00:02,000\r\n"));
        assert!(rendered.contains("Hola <i>mundo</i>\r\n\r\n99\r\n"));
        assert!(rendered.contains("00:00:03,000 --> 00:00:04,000\r\nSegunda linea\r\n\r\n"));
    }

    #[test]
    fn ass_render_uses_subparse_and_restores_override_blocks_and_line_breaks() {
        let ass = concat!(
            "[Script Info]\n",
            "Title: Demo\n\n",
            "[V4+ Styles]\n",
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
            "Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1\n\n",
            "[Events]\n",
            "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
            "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,banner,Hello {\\i1}world{\\i0}\\NNext\n",
        );
        let chunks =
            parse_subtitle_text(ass, DocumentFormat::Ass, ContentFormat::Ass, 100).expect("parse");
        let after = chunks[0]
            .source_text
            .replace("Hello", "Hola")
            .replace("world", "mundo")
            .replace("Next", "Siguiente");
        let rendered_chunk = rendered_chunk(&chunks[0], &after);

        let rendered =
            String::from_utf8(render_ass_document(ass, &[rendered_chunk]).expect("render ass"))
                .expect("utf8 ass");

        assert!(rendered.contains("[V4+ Styles]"));
        assert!(rendered.contains(",banner,Hola {\\i1}mundo{\\i0}\\NSiguiente"));
    }

    #[test]
    fn lrc_render_patches_lyrics_without_reordering_metadata_or_time_tags() {
        let lrc = concat!(
            "[ti: Song]\n",
            "[ar: Artist]\n\n",
            "[00:01.00][00:02.00]Hello\n",
            "plain lyric\n",
            "[:] keep this comment\n",
            "[00:03.00]Second\n",
        );
        let chunks = parse_lrc_text(lrc, 100).expect("parse lrc");
        let after = "<it0>Hola</it0>\n<it1>letra simple</it1>\n<it2>Segundo</it2>";
        let rendered_chunk = rendered_chunk(&chunks[0], after);

        let rendered = render_lrc_document(lrc, &[rendered_chunk]).expect("render lrc");

        assert!(rendered.starts_with("[ti: Song]\n[ar: Artist]"));
        assert!(rendered.contains("[00:01.00][00:02.00]Hola"));
        assert!(rendered.contains("\nletra simple\n"));
        assert!(rendered.contains("[:] keep this comment"));
        assert!(rendered.contains("[00:03.00]Segundo"));
    }

    #[test]
    fn restore_fails_when_translated_timed_unit_tags_are_missing() {
        let srt = concat!("1\n", "00:00:01,000 --> 00:00:02,000\n", "Hello\n\n",);
        let chunks =
            parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100).expect("parse");
        let map = crate::document_parsing::parse_map(&chunks[0].map_json).expect("map");

        let error = restore_timed_text_chunk(&map, "Hola").expect_err("missing unit tag");

        assert!(error.contains("missing expected unit tag"));
    }

    #[test]
    fn render_fails_when_timed_unit_reference_is_out_of_range() {
        let srt = concat!("1\n", "00:00:01,000 --> 00:00:02,000\n", "Hello\n\n",);
        let chunks =
            parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100).expect("parse");
        let mut chunk = rendered_chunk(&chunks[0], "<it0>Hola</it0>");
        let mut map = crate::document_parsing::parse_map(&chunk.map_json).expect("map");
        for entry in &mut map.entries {
            if entry.kind == TIMED_UNIT_KIND {
                entry.native_ref = Some("entry:99".into());
            }
        }
        chunk.map_json = map.to_json().expect("map json");

        let error = render_srt_document(srt, &[chunk]).expect_err("range mismatch");

        assert!(error.contains("does not exist in source"));
    }

    fn rendered_chunk(chunk: &ParsedChunk, after_translate_text: &str) -> RenderedChunk {
        let map = crate::document_parsing::parse_map(&chunk.map_json).expect("map");
        let translated_text =
            restore_timed_text_chunk(&map, after_translate_text).expect("restore timed chunk");
        RenderedChunk {
            sequence: chunk.sequence,
            source_text: chunk.source_text.clone(),
            after_translate_text: after_translate_text.into(),
            translated_text,
            map_json: chunk.map_json.clone(),
        }
    }
}
