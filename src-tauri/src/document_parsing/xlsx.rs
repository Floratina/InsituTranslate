use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};

use calamine::{open_workbook, Reader as CalamineReader, Xlsx};
use quick_xml::events::{BytesText, Event};
use quick_xml::{Reader, Writer};
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderEntry, PlaceholderMap, RenderInput};
use super::{
    chunk_raw_block_refs, chunk_raw_block_refs_with_progress, token_limit_usize, ChunkedRawBlock,
    DocumentParser, RawBlockRef,
};

const SHARED_STRINGS_XML: &str = "xl/sharedStrings.xml";
const XLSX_CHUNK_KIND: &str = "xlsx-shared-string-chunk";
const XLSX_BLOCK_KIND: &str = "xlsx-shared-string";
const XLSX_RUN_KIND: &str = "xlsx-shared-text";

pub struct XlsxParser;

impl DocumentParser for XlsxParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        validate_xlsx(input.source_path)?;
        let xml = match read_zip_text(input.source_path, SHARED_STRINGS_XML) {
            Ok(xml) => xml,
            Err(error) if error.contains("File not found") => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let strings = extract_shared_strings(&xml)?;
        let raw_blocks = strings
            .into_iter()
            .filter(|shared_string| !shared_string.text().trim().is_empty())
            .map(|shared_string| {
                RawBlockRef::new(
                    shared_string.text(),
                    shared_string.is_breakable(),
                    XlsxStringMeta {
                        string_index: shared_string.index,
                        nodes: shared_string.nodes,
                    },
                )
            })
            .collect::<Vec<_>>();
        let chunked_blocks = match input.progress {
            Some(progress) => chunk_raw_block_refs_with_progress(
                raw_blocks,
                token_limit_usize(input.token_limit),
                Some(progress),
            ),
            None => chunk_raw_block_refs(raw_blocks, token_limit_usize(input.token_limit)),
        };
        chunked_blocks
            .into_iter()
            .enumerate()
            .map(|(sequence, blocks)| xlsx_chunk_from_blocks(sequence, blocks))
            .collect()
    }

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        let map = super::parse_map(map_json)?;
        if map.block_ref.kind == XLSX_CHUNK_KIND {
            return restore_xlsx_text_chunk(&map, after_translate_text);
        }
        if map.block_ref.kind == XLSX_BLOCK_KIND {
            return Ok(strip_text_placeholders(after_translate_text));
        }
        super::placeholders::restore_from_json(map_json, after_translate_text)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(input.source_path)
            .map_err(|error| format!("Unable to read XLSX for render: {error}"))?;
        let reader = Cursor::new(bytes);
        let mut archive = ZipArchive::new(reader)
            .map_err(|error| format!("Unable to open XLSX for render: {error}"))?;
        let mut output = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut output);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let replacements = xlsx_replacements(input.chunks)?;

        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().to_string();
            writer
                .start_file(&name, options)
                .map_err(|error| error.to_string())?;
            if name == SHARED_STRINGS_XML {
                let mut xml = String::new();
                file.read_to_string(&mut xml)
                    .map_err(|error| error.to_string())?;
                let rendered = replace_shared_strings(&xml, &replacements)?;
                writer
                    .write_all(rendered.as_bytes())
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextNode {
    index: usize,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SharedString {
    index: usize,
    nodes: Vec<TextNode>,
}

impl SharedString {
    fn text(&self) -> String {
        self.nodes
            .iter()
            .map(|node| node.text.as_str())
            .collect::<String>()
    }

    fn is_breakable(&self) -> bool {
        self.nodes.len() <= 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XlsxStringMeta {
    string_index: usize,
    nodes: Vec<TextNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplacementText {
    Whole(String),
    Nodes(BTreeMap<usize, String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SharedStringReplacement {
    index: usize,
    text: ReplacementText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XlsxUnitDescriptor {
    tag: String,
    string_index: usize,
}

fn validate_xlsx(path: &std::path::Path) -> Result<(), String> {
    let mut workbook: Xlsx<_> = open_workbook(path)
        .map_err(|error| format!("Unable to open XLSX with calamine: {error}"))?;
    let sheet_names = workbook.sheet_names().to_owned();
    for sheet in sheet_names {
        workbook.worksheet_range(&sheet).map_err(|error| {
            format!("Unable to read XLSX sheet `{sheet}` with calamine: {error}")
        })?;
    }
    Ok(())
}

fn read_zip_text(path: &std::path::Path, entry: &str) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("Unable to open ZIP source: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut file = archive.by_name(entry).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

fn extract_shared_strings(xml: &str) -> Result<Vec<SharedString>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut strings = Vec::new();
    let mut in_si = false;
    let mut in_text = false;
    let mut string_index = 0_usize;
    let mut text_index = 0_usize;
    let mut nodes = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?
        {
            Event::Start(event) if local_name(event.name().as_ref()) == b"si" => {
                in_si = true;
                text_index = 0;
                nodes.clear();
            }
            Event::End(event) if local_name(event.name().as_ref()) == b"si" => {
                if in_si {
                    strings.push(SharedString {
                        index: string_index,
                        nodes: nodes.clone(),
                    });
                    string_index += 1;
                }
                in_si = false;
            }
            Event::Start(event) if in_si && local_name(event.name().as_ref()) == b"t" => {
                in_text = true;
            }
            Event::End(event) if local_name(event.name().as_ref()) == b"t" => {
                in_text = false;
            }
            Event::Text(text) if in_si && in_text => {
                nodes.push(TextNode {
                    index: text_index,
                    text: text
                        .unescape()
                        .map_err(|error| error.to_string())?
                        .into_owned(),
                });
                text_index += 1;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(strings)
}

fn xlsx_chunk_from_blocks(
    sequence: usize,
    blocks: Vec<ChunkedRawBlock<XlsxStringMeta>>,
) -> Result<ParsedChunk, String> {
    let mut source_parts = Vec::new();
    let mut preprocessed_parts = Vec::new();
    let mut entries = Vec::new();
    let mut next_run_index = 1_usize;

    for (local_index, block) in blocks.iter().enumerate() {
        let unit_id = format!("it{local_index}");
        let raw_text = block.text.clone();
        let (source_text, mut run_entries) =
            xlsx_source_for_block(block, &unit_id, &mut next_run_index);
        source_parts.push(format!("<{unit_id}>{source_text}</{unit_id}>"));
        preprocessed_parts.push(raw_text.clone());
        entries.push(PlaceholderEntry {
            id: unit_id,
            kind: XLSX_BLOCK_KIND.into(),
            original: raw_text,
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!("string:{}", block.metadata.string_index)),
        });
        entries.append(&mut run_entries);
    }

    let map = PlaceholderMap {
        version: super::types::PLACEHOLDER_MAP_VERSION,
        format: DocumentFormat::Xlsx,
        content_format: ContentFormat::Xml,
        block_ref: BlockRef {
            kind: XLSX_CHUNK_KIND.into(),
            path: Some(SHARED_STRINGS_XML.into()),
            index: Some(sequence),
            pointer: Some(format!("strings:{}", blocks.len())),
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

fn xlsx_source_for_block(
    block: &ChunkedRawBlock<XlsxStringMeta>,
    unit_id: &str,
    next_run_index: &mut usize,
) -> (String, Vec<PlaceholderEntry>) {
    if block.metadata.nodes.len() <= 1
        || block.source_start != 0
        || block.source_end != block.metadata.nodes_text_len()
    {
        return (block.text.clone(), Vec::new());
    }

    let mut source = String::new();
    let mut entries = Vec::new();
    for node in &block.metadata.nodes {
        let id = format!("t{}", *next_run_index);
        *next_run_index += 1;
        source.push_str(&format!("<{id}>{}</{id}>", node.text));
        entries.push(PlaceholderEntry {
            id,
            kind: XLSX_RUN_KIND.into(),
            original: node.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!("unit:{unit_id};text:{}", node.index)),
        });
    }
    (source, entries)
}

impl XlsxStringMeta {
    fn nodes_text_len(&self) -> usize {
        self.nodes.iter().map(|node| node.text.len()).sum()
    }
}

#[cfg(test)]
fn map_for_shared_string(index: usize, nodes: &[TextNode]) -> Result<PlaceholderMap, String> {
    let mut map = PlaceholderMap::empty(
        DocumentFormat::Xlsx,
        ContentFormat::Xml,
        BlockRef {
            kind: XLSX_BLOCK_KIND.into(),
            path: Some(SHARED_STRINGS_XML.into()),
            index: Some(index),
            pointer: None,
            prefix: String::new(),
            suffix: String::new(),
        },
    );

    if nodes.len() > 1 {
        map.entries = nodes
            .iter()
            .enumerate()
            .map(|(placeholder_index, node)| PlaceholderEntry {
                id: format!("t{}", placeholder_index + 1),
                kind: XLSX_RUN_KIND.into(),
                original: node.text.clone(),
                open: String::new(),
                close: String::new(),
                translatable: true,
                native_ref: Some(format!("text:{}", node.index)),
            })
            .collect();
    }
    Ok(map)
}

#[cfg(test)]
fn source_text_for_nodes(nodes: &[TextNode]) -> String {
    if nodes.len() <= 1 {
        return nodes
            .first()
            .map(|node| node.text.clone())
            .unwrap_or_default();
    }
    nodes
        .iter()
        .enumerate()
        .map(|(placeholder_index, node)| {
            let id = format!("t{}", placeholder_index + 1);
            format!("<{id}>{}</{id}>", node.text)
        })
        .collect::<String>()
}

fn xlsx_replacements(
    chunks: &[super::types::RenderedChunk],
) -> Result<Vec<SharedStringReplacement>, String> {
    let mut collected = Vec::<(i64, usize, usize, ReplacementText)>::new();
    let mut order = 0_usize;
    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        match map.block_ref.kind.as_str() {
            XLSX_CHUNK_KIND if map.block_ref.path.as_deref() == Some(SHARED_STRINGS_XML) => {
                let units = xlsx_unit_descriptors(&map)?;
                let tagged_text = if chunk.after_translate_text.contains("<it") {
                    chunk.after_translate_text.as_str()
                } else {
                    chunk.translated_text.as_str()
                };
                let translated_units = extract_xlsx_unit_text(tagged_text, &units)?;
                for (unit, translated_text) in units.into_iter().zip(translated_units) {
                    let run_entries = run_entries_for_unit(&map, &unit.tag);
                    let text = tagged_node_replacements_for_entries(&run_entries, &translated_text)
                        .map(ReplacementText::Nodes)
                        .unwrap_or_else(|| {
                            ReplacementText::Whole(strip_text_placeholders(&translated_text))
                        });
                    collected.push((chunk.sequence, order, unit.string_index, text));
                    order += 1;
                }
            }
            XLSX_BLOCK_KIND if map.block_ref.path.as_deref() == Some(SHARED_STRINGS_XML) => {
                let Some(index) = map.block_ref.index else {
                    continue;
                };
                let text = tagged_node_replacements(&map, &chunk.after_translate_text)
                    .map(ReplacementText::Nodes)
                    .unwrap_or_else(|| {
                        ReplacementText::Whole(strip_text_placeholders(&chunk.translated_text))
                    });
                collected.push((chunk.sequence, order, index, text));
                order += 1;
            }
            _ => {}
        }
    }
    collected.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut merged = BTreeMap::<usize, ReplacementText>::new();
    for (_, _, index, text) in collected {
        match merged.get_mut(&index) {
            Some(existing) => merge_replacement_text(existing, text),
            None => {
                merged.insert(index, text);
            }
        }
    }
    Ok(merged
        .into_iter()
        .map(|(index, text)| SharedStringReplacement { index, text })
        .collect())
}

fn tagged_node_replacements(
    map: &PlaceholderMap,
    translated: &str,
) -> Option<BTreeMap<usize, String>> {
    let entries = map
        .entries
        .iter()
        .filter(|entry| entry.kind == XLSX_RUN_KIND)
        .collect::<Vec<_>>();
    tagged_node_replacements_for_entries(&entries, translated)
}

fn tagged_node_replacements_for_entries(
    entries: &[&PlaceholderEntry],
    translated: &str,
) -> Option<BTreeMap<usize, String>> {
    if entries.is_empty() {
        return None;
    }

    let mut replacements = BTreeMap::new();
    for entry in entries {
        let text_index = text_index_from_native_ref(entry.native_ref.as_deref()?)?;
        let text = extract_tagged_text(translated, &entry.id)?;
        replacements.insert(text_index, text);
    }
    Some(replacements)
}

fn restore_xlsx_text_chunk(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    let units = xlsx_unit_descriptors(map)?;
    let translated_units = extract_xlsx_unit_text(after_translate_text, &units)?;
    let mut restored = Vec::new();
    for (unit, text) in units.into_iter().zip(translated_units) {
        let unit_text = if run_entries_for_unit(map, &unit.tag).is_empty() {
            text
        } else {
            strip_text_placeholders(&text)
        };
        restored.push(format!("<{}>{}</{}>", unit.tag, unit_text, unit.tag));
    }
    Ok(restored.join("\n"))
}

fn xlsx_unit_descriptors(map: &PlaceholderMap) -> Result<Vec<XlsxUnitDescriptor>, String> {
    let mut units = Vec::new();
    for entry in &map.entries {
        if entry.kind != XLSX_BLOCK_KIND {
            continue;
        }
        let string_index = entry
            .native_ref
            .as_deref()
            .and_then(|native_ref| native_ref.strip_prefix("string:"))
            .and_then(|index| index.parse::<usize>().ok())
            .ok_or_else(|| format!("XLSX unit {} is missing shared string reference", entry.id))?;
        units.push(XlsxUnitDescriptor {
            tag: entry.id.clone(),
            string_index,
        });
    }
    if units.is_empty() {
        return Err("XLSX chunk has no shared string entries".into());
    }
    Ok(units)
}

fn extract_xlsx_unit_text(text: &str, units: &[XlsxUnitDescriptor]) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for unit in units {
        let value = extract_tagged_text(text, &unit.tag).ok_or_else(|| {
            format!(
                "Translated XLSX chunk is missing expected unit tag <{}>",
                unit.tag
            )
        })?;
        values.push(value);
    }
    Ok(values)
}

fn run_entries_for_unit<'a>(map: &'a PlaceholderMap, unit_tag: &str) -> Vec<&'a PlaceholderEntry> {
    map.entries
        .iter()
        .filter(|entry| {
            entry.kind == XLSX_RUN_KIND
                && entry
                    .native_ref
                    .as_deref()
                    .is_some_and(|native_ref| native_ref.starts_with(&format!("unit:{unit_tag};")))
        })
        .collect()
}

fn text_index_from_native_ref(native_ref: &str) -> Option<usize> {
    native_ref
        .strip_prefix("text:")
        .or_else(|| {
            native_ref
                .split(';')
                .find_map(|part| part.strip_prefix("text:"))
        })
        .and_then(|index| index.parse::<usize>().ok())
}

fn merge_replacement_text(existing: &mut ReplacementText, next: ReplacementText) {
    match (existing, next) {
        (ReplacementText::Whole(left), ReplacementText::Whole(right)) => left.push_str(&right),
        (ReplacementText::Nodes(left), ReplacementText::Nodes(right)) => {
            for (index, value) in right {
                left.entry(index)
                    .and_modify(|existing| existing.push_str(&value))
                    .or_insert(value);
            }
        }
        (ReplacementText::Whole(left), ReplacementText::Nodes(right)) => {
            left.push_str(&right.into_values().collect::<String>());
        }
        (ReplacementText::Nodes(left), ReplacementText::Whole(right)) => {
            if let Some((_, value)) = left.iter_mut().next_back() {
                value.push_str(&right);
            } else {
                left.insert(0, right);
            }
        }
    }
}

fn extract_tagged_text(text: &str, id: &str) -> Option<String> {
    let pattern = Regex::new(&format!(
        r"(?is)<\s*{}\s*>(.*?)<\s*/\s*{}\s*>",
        regex::escape(id),
        regex::escape(id)
    ))
    .ok()?;
    let mut captures = pattern.captures_iter(text);
    let first = captures.next()?;
    if captures.next().is_some() {
        return None;
    }
    first.get(1).map(|value| value.as_str().to_string())
}

fn strip_text_placeholders(text: &str) -> String {
    Regex::new(r"(?is)</?\s*t\d+\s*>")
        .expect("static XLSX text placeholder regex")
        .replace_all(text, "")
        .to_string()
}

fn replace_shared_strings(
    xml: &str,
    replacements: &[SharedStringReplacement],
) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut string_index = 0_usize;
    let mut text_index = 0_usize;
    let mut in_target_si = false;
    let mut in_text = false;
    let mut replacement = None::<&ReplacementText>;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?;
        match event {
            Event::Start(ref event) if local_name(event.name().as_ref()) == b"si" => {
                replacement = replacements
                    .iter()
                    .find(|item| item.index == string_index)
                    .map(|item| &item.text);
                in_target_si = replacement.is_some();
                text_index = 0;
                writer
                    .write_event(Event::Start(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::End(ref event) if local_name(event.name().as_ref()) == b"si" => {
                string_index += 1;
                in_target_si = false;
                replacement = None;
                writer
                    .write_event(Event::End(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::Start(ref event)
                if in_target_si && local_name(event.name().as_ref()) == b"t" =>
            {
                in_text = true;
                writer
                    .write_event(Event::Start(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::End(ref event) if local_name(event.name().as_ref()) == b"t" => {
                in_text = false;
                writer
                    .write_event(Event::End(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::Text(ref text) if in_target_si && in_text => {
                let value = replacement
                    .map(|replacement| replacement_value(replacement, text_index))
                    .unwrap_or_default();
                writer
                    .write_event(Event::Text(BytesText::new(&value)))
                    .map_err(|error| error.to_string())?;
                text_index += 1;
            }
            Event::Eof => break,
            other => writer
                .write_event(other)
                .map_err(|error| error.to_string())?,
        }
        buf.clear();
    }
    String::from_utf8(writer.into_inner()).map_err(|error| error.to_string())
}

fn replacement_value(replacement: &ReplacementText, text_index: usize) -> String {
    match replacement {
        ReplacementText::Whole(text) => {
            if text_index == 0 {
                text.clone()
            } else {
                String::new()
            }
        }
        ReplacementText::Nodes(nodes) => nodes.get(&text_index).cloned().unwrap_or_default(),
    }
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parsing::types::{ParserInput, RenderedChunk};

    #[test]
    fn extracts_shared_strings_with_rich_text_placeholders() {
        let xml = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Hello</t></si><si><r><t>Rich </t></r><r><t>Text</t></r></si></sst>"#;
        let strings = extract_shared_strings(xml).expect("extract strings");

        assert_eq!(source_text_for_nodes(&strings[0].nodes), "Hello");
        assert_eq!(
            source_text_for_nodes(&strings[1].nodes),
            "<t1>Rich </t1><t2>Text</t2>"
        );
    }

    #[test]
    fn replaces_shared_string_text_nodes_only() {
        let xml = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Hello</t></si><si><r><rPr><b /></rPr><t>Rich </t></r><r><t>Text</t></r></si></sst>"#;
        let replacement = SharedStringReplacement {
            index: 1,
            text: ReplacementText::Nodes(BTreeMap::from([
                (0, "Texto ".to_string()),
                (1, "Rico".to_string()),
            ])),
        };

        let rendered = replace_shared_strings(xml, &[replacement]).expect("replace shared strings");

        assert!(rendered.contains("<b"));
        assert!(rendered.contains(">Hello<"));
        assert!(rendered.contains(">Texto <"));
        assert!(rendered.contains(">Rico<"));
    }

    #[test]
    fn render_keeps_worksheet_xml_unchanged() {
        let path = temp_path("xlsx-render.xlsx");
        write_test_xlsx(&path).expect("write test xlsx");
        let original_sheet = read_zip_text(&path, "xl/worksheets/sheet1.xml").expect("sheet xml");
        let chunks = vec![rendered_xlsx_chunk(0, "Hello", "Hola", "Hola")];

        let output = XlsxParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &chunks,
            })
            .expect("render xlsx");
        let shared_strings =
            read_zip_entry_from_bytes(&output, SHARED_STRINGS_XML).expect("shared strings");
        let rendered_sheet =
            read_zip_entry_from_bytes(&output, "xl/worksheets/sheet1.xml").expect("sheet xml");

        assert!(shared_strings.contains(">Hola<"));
        assert_eq!(rendered_sheet, original_sheet);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parse_generates_rich_text_placeholders_after_chunking_only() {
        let path = temp_path("xlsx-parse.xlsx");
        write_test_xlsx_with_shared_strings(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><r><t>Rich </t></r><r><rPr><b /></rPr><t>Text</t></r></si></sst>"#,
        )
        .expect("write test xlsx");

        let chunks = XlsxParser
            .parse(ParserInput {
                source_path: &path,
                token_limit: 1,
                progress: None,
            })
            .expect("parse xlsx");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].preprocessed_text, "Rich Text");
        assert!(!chunks[0].preprocessed_text.contains("<t1>"));
        assert_eq!(
            chunks[0].source_text,
            "<it0><t1>Rich </t1><t2>Text</t2></it0>"
        );
        let _ = std::fs::remove_file(path);
    }

    fn rendered_xlsx_chunk(
        index: usize,
        source_text: &str,
        after_translate_text: &str,
        translated_text: &str,
    ) -> RenderedChunk {
        let nodes = vec![TextNode {
            index: 0,
            text: source_text.into(),
        }];
        RenderedChunk {
            sequence: index as i64,
            source_text: source_text.into(),
            after_translate_text: after_translate_text.into(),
            translated_text: translated_text.into(),
            map_json: map_for_shared_string(index, &nodes)
                .expect("map")
                .to_json()
                .expect("serialize map"),
        }
    }

    fn write_test_xlsx(path: &std::path::Path) -> Result<(), String> {
        write_test_xlsx_with_shared_strings(
            path,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>Hello</t></si></sst>"#,
        )
    }

    fn write_test_xlsx_with_shared_strings(
        path: &std::path::Path,
        shared_strings: &str,
    ) -> Result<(), String> {
        let entries = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><f>1+1</f><v>2</v></c></row></sheetData></worksheet>"#,
            ),
            (SHARED_STRINGS_XML, shared_strings),
        ];
        let file = File::create(path).map_err(|error| error.to_string())?;
        let mut writer = ZipWriter::new(file);
        for (name, text) in entries {
            writer
                .start_file(name, SimpleFileOptions::default())
                .map_err(|error| error.to_string())?;
            writer
                .write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        writer.finish().map_err(|error| error.to_string())?;
        Ok(())
    }

    fn read_zip_entry_from_bytes(bytes: &[u8], entry: &str) -> Result<String, String> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
        let mut file = archive.by_name(entry).map_err(|error| error.to_string())?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        Ok(text)
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("insitu-{nanos}-{name}"))
    }
}
