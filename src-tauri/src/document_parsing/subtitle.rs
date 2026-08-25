use std::collections::HashMap;

use lrc::{Lyrics, TimeTag};
use regex::Regex;

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::placeholders::{protect_html, restore_with_map};
use super::types::{
    BlockRef, ParsedChunk, ParserProgress, PlaceholderEntry, PlaceholderMap, RenderInput,
    RenderedChunk,
};
use super::{
    chunk_raw_block_refs, chunk_raw_block_refs_with_progress, token_limit_usize, ChunkedRawBlock,
    DocumentParser, RawBlockRef,
};

const TIMED_TEXT_CHUNK_KIND: &str = "timed-text-chunk";
const TIMED_UNIT_KIND: &str = "timed-unit";
const LEGACY_TIMED_TEXT_KIND: &str = "timed-text";
const ASS_CONTROL_KIND: &str = "ass-control";

pub struct SubtitleParser {
    pub format: DocumentFormat,
    pub content_format: ContentFormat,
}

impl DocumentParser for SubtitleParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read subtitle source: {error}"))?;
        let progress = input.progress;
        match self.format {
            DocumentFormat::Srt | DocumentFormat::Ass => parse_subtitle_text(
                &text,
                self.format,
                self.content_format,
                input.token_limit,
                progress,
            ),
            DocumentFormat::Lrc => parse_lrc_text(&text, input.token_limit, progress),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssFieldsInfo {
    start_field_index: usize,
    end_field_index: usize,
    text_field_index: usize,
    field_count: usize,
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
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let text_ranges = match format {
        DocumentFormat::Srt => srt_body_ranges(text)?,
        DocumentFormat::Ass => ass_dialogue_text_ranges(text)?,
        _ => return Err("Unsupported subtitle parser format".into()),
    };

    let mut units = Vec::new();
    for (index, range) in text_ranges.into_iter().enumerate() {
        let source_text = text
            .get(range.start..range.end)
            .ok_or_else(|| format!("Invalid timed text range for entry {index}"))?
            .to_string();
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

    build_timed_chunks(units, format, content_format, token_limit, progress)
}

fn parse_lrc_text(
    text: &str,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
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

    build_timed_chunks(
        units,
        DocumentFormat::Lrc,
        ContentFormat::Lrc,
        token_limit,
        progress,
    )
}

fn build_timed_chunks(
    units: Vec<TimedTextUnit>,
    format: DocumentFormat,
    content_format: ContentFormat,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
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
    let chunked_blocks = match progress {
        Some(progress) => chunk_raw_block_refs_with_progress(
            raw_blocks,
            token_limit_usize(token_limit),
            Some(progress),
        ),
        None => chunk_raw_block_refs(raw_blocks, token_limit_usize(token_limit)),
    };
    chunked_blocks
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
        if map.format == DocumentFormat::Ass {
            validate_ass_control_placeholders(map, &unit.tag, &translated_text)?;
        }
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

fn validate_ass_control_placeholders(
    map: &PlaceholderMap,
    unit_tag: &str,
    translated_text: &str,
) -> Result<(), String> {
    let mut previous_position = None;

    for entry in map
        .entries
        .iter()
        .filter(|entry| entry.kind == ASS_CONTROL_KIND)
    {
        let open = format!("<{}>", entry.id);
        let close = format!("</{}>", entry.id);
        let token = format!("{open}{close}");

        if placeholder_belongs_to_unit(entry, unit_tag) {
            let positions = translated_text.match_indices(&token).collect::<Vec<_>>();
            if positions.len() != 1
                || translated_text.matches(&open).count() != 1
                || translated_text.matches(&close).count() != 1
            {
                return Err(format!(
                    "Translated ASS unit <{unit_tag}> must contain control placeholder <{}> exactly once",
                    entry.id
                ));
            }
            let position = positions[0].0;
            if previous_position.is_some_and(|previous| position <= previous) {
                return Err(format!(
                    "Translated ASS unit <{unit_tag}> changed control placeholder order at <{}>",
                    entry.id
                ));
            }
            previous_position = Some(position);
        } else if translated_text.contains(&open) || translated_text.contains(&close) {
            return Err(format!(
                "Translated ASS unit <{unit_tag}> contains control placeholder <{}> owned by another subtitle",
                entry.id
            ));
        }
    }

    Ok(())
}

fn render_srt_document(text: &str, chunks: &[RenderedChunk]) -> Result<String, String> {
    let ranges = srt_body_ranges(text)?;

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
        let replacement = preserve_source_line_endings(
            &text[range.start..range.end],
            &replacement,
            &format!("translated SRT entry {entry_index}"),
        )?;
        patches.push(TextPatch { range, replacement });
    }
    apply_text_patches(text, patches)
}

fn render_ass_document(text: &str, chunks: &[RenderedChunk]) -> Result<Vec<u8>, String> {
    let ranges = ass_dialogue_text_ranges(text)?;
    let Some(translations) = collect_timed_translations(chunks, TimedTargetKind::Entry)? else {
        return Ok(legacy_or_original(text, chunks)?.into_bytes());
    };
    let mut patches = Vec::new();
    for (entry_index, replacement) in translations {
        let Some(range) = ranges.get(entry_index).copied() else {
            return Err(format!(
                "Translated ASS entry index {entry_index} does not exist in source"
            ));
        };
        patches.push(TextPatch { range, replacement });
    }
    apply_text_patches(text, patches).map(String::into_bytes)
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

fn preserve_source_line_endings(
    source: &str,
    replacement: &str,
    context: &str,
) -> Result<String, String> {
    let (source_lines, source_endings) = split_lines_and_endings(source);
    let (replacement_lines, _) = split_lines_and_endings(replacement);
    if source_lines.len() != replacement_lines.len() {
        return Err(format!(
            "Invalid {context}: expected {} text lines but received {}",
            source_lines.len(),
            replacement_lines.len()
        ));
    }

    let mut output = String::with_capacity(replacement.len());
    for (index, line) in replacement_lines.into_iter().enumerate() {
        output.push_str(line);
        if let Some(ending) = source_endings.get(index) {
            output.push_str(ending);
        }
    }
    Ok(output)
}

fn split_lines_and_endings(text: &str) -> (Vec<&str>, Vec<&str>) {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut endings = Vec::new();
    let mut start = 0_usize;

    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let ending_start = if index > start && bytes[index - 1] == b'\r' {
            index - 1
        } else {
            index
        };
        lines.push(&text[start..ending_start]);
        endings.push(&text[ending_start..index + 1]);
        start = index + 1;
    }
    lines.push(&text[start..]);

    (lines, endings)
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

        let index_line = lines[cursor];
        let index_text = line_text(text, index_line);
        let index_text = if index_line.start == 0 {
            index_text.strip_prefix('\u{feff}').unwrap_or(index_text)
        } else {
            index_text
        };
        if !is_ascii_unsigned_integer(index_text.trim()) {
            return Err(format!(
                "Invalid SRT subtitle index at source line {}",
                index_line.index + 1
            ));
        }

        cursor += 1;
        if cursor >= lines.len() {
            return Err(format!(
                "Invalid SRT block after source line {}: missing timestamp line",
                index_line.index + 1
            ));
        }
        validate_srt_timestamp_line(line_text(text, lines[cursor]), lines[cursor].index + 1)?;

        cursor += 1;
        let body_start_line = cursor;
        while cursor < lines.len() && !line_text(text, lines[cursor]).trim().is_empty() {
            cursor += 1;
        }
        if body_start_line == cursor {
            return Err(format!(
                "Invalid SRT block after source line {}: missing subtitle text",
                lines[body_start_line - 1].index + 1
            ));
        }

        let range = TextRange {
            start: lines[body_start_line].start,
            end: lines[cursor - 1].end,
        };
        validate_text_range(text, range, "SRT subtitle text")?;
        ranges.push(range);
    }

    Ok(ranges)
}

fn validate_srt_timestamp_line(line: &str, line_number: usize) -> Result<(), String> {
    let timestamp = line.trim();
    let Some((start, end)) = timestamp.split_once("-->") else {
        return Err(format!(
            "Invalid SRT timestamp at source line {line_number}: missing --> separator"
        ));
    };
    if end.contains("-->")
        || !is_valid_srt_timestamp(start.trim())
        || !is_valid_srt_timestamp(end.trim())
    {
        return Err(format!(
            "Invalid SRT timestamp syntax at source line {line_number}"
        ));
    }
    Ok(())
}

fn is_valid_srt_timestamp(timestamp: &str) -> bool {
    let Some((hours_minutes_seconds, milliseconds)) = timestamp.rsplit_once(',') else {
        return false;
    };
    let mut components = hours_minutes_seconds.split(':');
    let (Some(hours), Some(minutes), Some(seconds), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) else {
        return false;
    };

    is_ascii_unsigned_integer(hours)
        && is_two_digit_component(minutes, 59)
        && is_two_digit_component(seconds, 59)
        && milliseconds.len() == 3
        && is_ascii_unsigned_integer(milliseconds)
}

fn ass_dialogue_text_ranges(text: &str) -> Result<Vec<TextRange>, String> {
    let mut ranges = Vec::new();
    let mut in_events = false;
    let mut saw_events = false;
    let mut saw_events_format = false;
    let mut fields_info = None;

    for line in line_ranges(text) {
        let raw_line = line_text(text, line);
        let structural_line = if line.start == 0 {
            raw_line.strip_prefix('\u{feff}').unwrap_or(raw_line)
        } else {
            raw_line
        };
        let trimmed = structural_line.trim();

        if let Some(section_name) = ass_section_name(trimmed) {
            in_events = section_name.eq_ignore_ascii_case("Events");
            if in_events {
                saw_events = true;
                fields_info = None;
            }
            continue;
        }
        if !in_events || trimmed.is_empty() {
            continue;
        }

        if let Some(format_fields) = strip_ascii_label(trimmed, "Format:") {
            fields_info = Some(parse_ass_format(format_fields, line.index + 1)?);
            saw_events_format = true;
            continue;
        }

        if strip_ascii_label(trimmed, "Dialogue:").is_none() {
            continue;
        }

        let Some(info) = fields_info else {
            return Err(format!(
                "Invalid ASS Dialogue at source line {}: missing preceding Events Format",
                line.index + 1
            ));
        };
        ranges.push(parse_ass_dialogue_range(text, line, info)?);
    }

    if !saw_events {
        return Err("Invalid ASS document: missing [Events] section".into());
    }
    if !saw_events_format {
        return Err("Invalid ASS document: missing Format line in [Events]".into());
    }

    Ok(ranges)
}

fn ass_section_name(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.strip_suffix(']')
}

fn strip_ascii_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let prefix = line.get(..label.len())?;
    prefix
        .eq_ignore_ascii_case(label)
        .then(|| &line[label.len()..])
}

fn parse_ass_format(format_fields: &str, line_number: usize) -> Result<AssFieldsInfo, String> {
    let fields = format_fields.split(',').map(str::trim).collect::<Vec<_>>();
    let start_field_index = unique_ass_field_index(&fields, "Start", line_number)?;
    let end_field_index = unique_ass_field_index(&fields, "End", line_number)?;
    let text_field_index = unique_ass_field_index(&fields, "Text", line_number)?;

    if text_field_index + 1 != fields.len() {
        return Err(format!(
            "Invalid ASS Format at source line {line_number}: Text must be the final field"
        ));
    }

    Ok(AssFieldsInfo {
        start_field_index,
        end_field_index,
        text_field_index,
        field_count: fields.len(),
    })
}

fn unique_ass_field_index(
    fields: &[&str],
    required: &str,
    line_number: usize,
) -> Result<usize, String> {
    let matches = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| field.eq_ignore_ascii_case(required).then_some(index))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(format!(
            "Invalid ASS Format at source line {line_number}: missing {required} field"
        )),
        _ => Err(format!(
            "Invalid ASS Format at source line {line_number}: duplicate {required} field"
        )),
    }
}

fn parse_ass_dialogue_range(
    text: &str,
    line: LineRange,
    info: AssFieldsInfo,
) -> Result<TextRange, String> {
    let raw_line = line_text(text, line);
    let leading_whitespace = leading_whitespace_len(raw_line);
    let labelled_line = &raw_line[leading_whitespace..];
    let Some(fields_text) = strip_ascii_label(labelled_line, "Dialogue:") else {
        return Err(format!(
            "Invalid ASS Dialogue label at source line {}",
            line.index + 1
        ));
    };
    let fields_offset = leading_whitespace + "Dialogue:".len();
    let fields_text_offset = leading_whitespace_len(fields_text);
    let fields_text = &fields_text[fields_text_offset..];
    let fields_start = fields_offset + fields_text_offset;

    let mut fields = Vec::with_capacity(info.field_count);
    let mut cursor = 0_usize;
    for _ in 0..info.text_field_index {
        let Some(comma_offset) = fields_text[cursor..].find(',') else {
            return Err(format!(
                "Invalid ASS Dialogue at source line {}: expected {} fields before Text",
                line.index + 1,
                info.text_field_index
            ));
        };
        let comma = cursor + comma_offset;
        fields.push(&fields_text[cursor..comma]);
        cursor = comma + 1;
    }
    fields.push(&fields_text[cursor..]);

    for (field_name, field_index) in [
        ("Start", info.start_field_index),
        ("End", info.end_field_index),
    ] {
        let Some(value) = fields.get(field_index) else {
            return Err(format!(
                "Invalid ASS Dialogue at source line {}: missing {field_name} field",
                line.index + 1
            ));
        };
        if !is_valid_ass_timestamp(value.trim()) {
            return Err(format!(
                "Invalid ASS {field_name} timestamp at source line {}",
                line.index + 1
            ));
        }
    }

    let range = TextRange {
        start: line.start + fields_start + cursor,
        end: line.end,
    };
    validate_text_range(text, range, "ASS Dialogue Text")?;
    Ok(range)
}

fn is_valid_ass_timestamp(timestamp: &str) -> bool {
    let Some((hours_minutes_seconds, centiseconds)) = timestamp
        .rsplit_once('.')
        .or_else(|| timestamp.rsplit_once(':'))
    else {
        return false;
    };
    let mut components = hours_minutes_seconds.split(':');
    let (Some(hours), Some(minutes), Some(seconds), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) else {
        return false;
    };

    is_ascii_unsigned_integer(hours)
        && is_two_digit_component(minutes, 59)
        && is_two_digit_component(seconds, 59)
        && centiseconds.len() == 2
        && is_ascii_unsigned_integer(centiseconds)
}

fn is_ascii_unsigned_integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_two_digit_component(value: &str, maximum: u8) -> bool {
    value.len() == 2
        && is_ascii_unsigned_integer(value)
        && value.parse::<u8>().is_ok_and(|parsed| parsed <= maximum)
}

fn validate_text_range(text: &str, range: TextRange, context: &str) -> Result<(), String> {
    if range.start > range.end
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return Err(format!(
            "Invalid {context} byte range {}:{}",
            range.start, range.end
        ));
    }
    Ok(())
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

        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 2, None)
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
        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100, None)
            .expect("parse");
        let after = "<it0>Hola <t1>mundo</t1></it0>\n<it1>Segunda linea</it1>";
        let rendered_chunk = rendered_chunk(&chunks[0], after);

        let rendered = render_srt_document(srt, &[rendered_chunk]).expect("render srt");

        assert!(rendered.contains("42\r\n00:00:01,000 --> 00:00:02,000\r\n"));
        assert!(rendered.contains("Hola <i>mundo</i>\r\n\r\n99\r\n"));
        assert!(rendered.contains("00:00:03,000 --> 00:00:04,000\r\nSegunda linea\r\n\r\n"));
    }

    #[test]
    fn srt_accepts_bom_unicode_and_arrow_whitespace_with_byte_exact_patching() {
        let srt = concat!(
            "\u{feff}7\r\n",
            "00:00:01,000   -->\t00:00:02,000\r\n",
            "你好 😀\r\n",
            "<b>second</b>\r\n\r\n",
            "42\r\n",
            "12:34:56,789 --> 12:34:58,001\r\n",
            "尾声\r\n\r\n",
        );
        let ranges = srt_body_ranges(srt).expect("srt ranges");
        assert!(ranges
            .iter()
            .all(|range| srt.is_char_boundary(range.start) && srt.is_char_boundary(range.end)));

        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100, None)
            .expect("parse srt");
        let after = "<it0>译文 😀\n<t1>标签</t1></it0>\n<it1>结尾</it1>";
        let rendered =
            render_srt_document(srt, &[rendered_chunk(&chunks[0], after)]).expect("render srt");
        let expected = concat!(
            "\u{feff}7\r\n",
            "00:00:01,000   -->\t00:00:02,000\r\n",
            "译文 😀\r\n",
            "<b>标签</b>\r\n\r\n",
            "42\r\n",
            "12:34:56,789 --> 12:34:58,001\r\n",
            "结尾\r\n\r\n",
        );

        assert_eq!(rendered.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn srt_rejects_invalid_indices_timestamps_and_incomplete_blocks() {
        let cases = [
            (
                "word\n00:00:01,000 --> 00:00:02,000\nText\n",
                "subtitle index",
            ),
            ("1\nmissing arrow\nText\n", "missing -->"),
            (
                "1\n00:60:01,000 --> 00:00:02,000\nText\n",
                "timestamp syntax",
            ),
            (
                "1\n00:00:01,000 --> 00:00:02,000\n",
                "missing subtitle text",
            ),
            ("1\n", "missing timestamp line"),
        ];

        for (source, expected_error) in cases {
            let error = srt_body_ranges(source).expect_err("invalid SRT must fail");
            assert!(
                error.contains(expected_error),
                "expected {expected_error:?} in {error:?}"
            );
        }
    }

    #[test]
    fn srt_rejects_translation_that_changes_multiline_structure() {
        let srt = concat!(
            "1\r\n",
            "00:00:01,000 --> 00:00:02,000\r\n",
            "First\r\n",
            "Second\r\n\r\n",
        );
        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100, None)
            .expect("parse SRT");
        let rendered = rendered_chunk(&chunks[0], "<it0>Only one line</it0>");

        let error = render_srt_document(srt, &[rendered]).expect_err("line count mismatch");

        assert!(error.contains("expected 2 text lines but received 1"));
    }

    #[test]
    fn ass_uses_dynamic_text_position_and_patches_only_text_bytes() {
        let ass = concat!(
            "\u{feff}[Script Info]\r\n",
            "Title: 字幕 😀\r\n\r\n",
            "[V4+ Styles]\r\n",
            "Style: Default,Arial,20\r\n\r\n",
            "[Events]\r\n",
            "Format: Start, Layer, End, Style, Text\r\n",
            "Comment: 0:00:00.00,8,0:00:10.00,Default,do not translate, ever\r\n",
            "Dialogue: 0:00:01.00,7,0:00:02.00,Default,你好, friend 😀  \r\n",
            "[Fonts]\r\n",
            "fontname: demo.ttf\r\n",
            "0123456789abcdef\r\n",
        );
        let ranges = ass_dialogue_text_ranges(ass).expect("ASS ranges");
        assert_eq!(ranges.len(), 1);
        assert_eq!(&ass[ranges[0].start..ranges[0].end], "你好, friend 😀  ");
        assert!(ass.is_char_boundary(ranges[0].start));
        assert!(ass.is_char_boundary(ranges[0].end));

        let chunks = parse_subtitle_text(ass, DocumentFormat::Ass, ContentFormat::Ass, 100, None)
            .expect("parse ASS");
        let rendered = String::from_utf8(
            render_ass_document(
                ass,
                &[rendered_chunk(&chunks[0], "<it0>译文, comma 🌸  </it0>")],
            )
            .expect("render ASS"),
        )
        .expect("UTF-8 ASS");
        let expected = ass.replacen("你好, friend 😀  ", "译文, comma 🌸  ", 1);

        assert_eq!(rendered.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn ass_restores_override_blocks_and_escape_sequences_byte_exactly() {
        let ass = concat!(
            "[Script Info]\n",
            "Title: Demo\n\n",
            "[V4+ Styles]\n",
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
            "Style: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1\n\n",
            "[Events]\n",
            "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
            "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,banner,中😀{\\i1}{\\bord2\\shad1}Hello{\\i0}\\Nnext\\nsoft\\hspace\n",
        );
        let chunks = parse_subtitle_text(ass, DocumentFormat::Ass, ContentFormat::Ass, 100, None)
            .expect("parse");
        let after = chunks[0]
            .source_text
            .replace("Hello", "Hola")
            .replace("next", "siguiente")
            .replace("soft", "suave")
            .replace("space", "espacio");
        let rendered_chunk = rendered_chunk(&chunks[0], &after);

        let rendered =
            String::from_utf8(render_ass_document(ass, &[rendered_chunk]).expect("render ass"))
                .expect("utf8 ass");

        let expected = ass.replacen(
            "中😀{\\i1}{\\bord2\\shad1}Hello{\\i0}\\Nnext\\nsoft\\hspace",
            "中😀{\\i1}{\\bord2\\shad1}Hola{\\i0}\\Nsiguiente\\nsuave\\hespacio",
            1,
        );
        assert_eq!(rendered.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn ass_rejects_invalid_formats_dialogues_and_timestamps() {
        let cases = [
            ("[Script Info]\nTitle: Demo\n", "missing [Events] section"),
            (
                "[Events]\nDialogue: 0,0:00:01.00,0:00:02.00,Text\n",
                "missing preceding Events Format",
            ),
            (
                "[Events]\nFormat: Start, End, Text, Text\n",
                "duplicate Text field",
            ),
            (
                "[Events]\nFormat: Start, Start, End, Text\n",
                "duplicate Start field",
            ),
            (
                "[Events]\nFormat: Start, End, End, Text\n",
                "duplicate End field",
            ),
            ("[Events]\nFormat: Start, Text\n", "missing End field"),
            ("[Events]\nFormat: End, Text\n", "missing Start field"),
            (
                "[Events]\nFormat: Start, End, Style\n",
                "missing Text field",
            ),
            (
                "[Events]\nFormat: Start, End, Text, Style\n",
                "Text must be the final field",
            ),
            (
                "[Events]\nFormat: Start, End, Style, Text\nDialogue: 0:00:01.00,0:00:02.00\n",
                "expected 3 fields before Text",
            ),
            (
                "[Events]\nFormat: Start, End, Text\nDialogue: bad,0:00:02.00,Text\n",
                "Start timestamp",
            ),
            (
                "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:01.00,0:99:02.00,Text\n",
                "End timestamp",
            ),
        ];

        for (source, expected_error) in cases {
            let error = ass_dialogue_text_ranges(source).expect_err("invalid ASS must fail");
            assert!(
                error.contains(expected_error),
                "expected {expected_error:?} in {error:?}"
            );
        }
    }

    #[test]
    fn ass_control_placeholders_must_not_be_removed_or_reordered() {
        let ass = concat!(
            "[Events]\n",
            "Format: Start, End, Text\n",
            "Dialogue: 0:00:01.00,0:00:02.00,前{\\i1}中{\\i0}后\\N尾\n",
        );
        let chunks = parse_subtitle_text(ass, DocumentFormat::Ass, ContentFormat::Ass, 100, None)
            .expect("parse ASS");
        let map = crate::document_parsing::parse_map(&chunks[0].map_json).expect("map");

        let missing = chunks[0].source_text.replacen("<t1></t1>", "", 1);
        assert!(restore_timed_text_chunk(&map, &missing).is_err());

        let reordered = chunks[0]
            .source_text
            .replacen("<t1></t1>", "__FIRST_CONTROL__", 1)
            .replacen("<t2></t2>", "<t1></t1>", 1)
            .replacen("__FIRST_CONTROL__", "<t2></t2>", 1);
        assert!(restore_timed_text_chunk(&map, &reordered).is_err());
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
        let chunks = parse_lrc_text(lrc, 100, None).expect("parse lrc");
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
        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100, None)
            .expect("parse");
        let map = crate::document_parsing::parse_map(&chunks[0].map_json).expect("map");

        let error = restore_timed_text_chunk(&map, "Hola").expect_err("missing unit tag");

        assert!(error.contains("missing expected unit tag"));
    }

    #[test]
    fn render_fails_when_timed_unit_reference_is_out_of_range() {
        let srt = concat!("1\n", "00:00:01,000 --> 00:00:02,000\n", "Hello\n\n",);
        let chunks = parse_subtitle_text(srt, DocumentFormat::Srt, ContentFormat::Srt, 100, None)
            .expect("parse");
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
