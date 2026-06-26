use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};

use quick_xml::events::{BytesText, Event};
use quick_xml::{Reader, Writer};
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderEntry, PlaceholderMap, RenderInput};
use super::{
    chunk_raw_block_refs, token_limit_usize, ChunkedRawBlock, DocumentParser, RawBlockRef,
};

const DOCUMENT_XML: &str = "word/document.xml";
const DOCX_CHUNK_KIND: &str = "docx-text-chunk";
const DOCX_BLOCK_KIND: &str = "docx-text-block";
const DOCX_RUN_KIND: &str = "docx-run-text";

pub struct DocxParser;

impl DocumentParser for DocxParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        validate_docx(input.source_path)?;
        let xml = read_zip_text(input.source_path, DOCUMENT_XML)?;
        let blocks = extract_word_text_blocks(&xml)?;
        let raw_blocks = blocks
            .into_iter()
            .filter(|block| !block.text().trim().is_empty())
            .map(|block| {
                RawBlockRef::new(
                    block.text(),
                    block.is_breakable(),
                    DocxBlockMeta {
                        block_index: block.index,
                        nodes: block.nodes,
                    },
                )
            })
            .collect::<Vec<_>>();
        chunk_raw_block_refs(raw_blocks, token_limit_usize(input.token_limit))
            .into_iter()
            .enumerate()
            .map(|(sequence, blocks)| docx_chunk_from_blocks(sequence, blocks))
            .collect()
    }

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        let map = super::parse_map(map_json)?;
        if map.block_ref.kind == DOCX_CHUNK_KIND {
            return restore_docx_text_chunk(&map, after_translate_text);
        }
        if map.block_ref.kind == DOCX_BLOCK_KIND {
            return Ok(strip_run_placeholders(after_translate_text));
        }
        super::placeholders::restore_from_json(map_json, after_translate_text)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(input.source_path)
            .map_err(|error| format!("Unable to read DOCX for render: {error}"))?;
        let reader = Cursor::new(bytes);
        let mut archive = ZipArchive::new(reader)
            .map_err(|error| format!("Unable to open DOCX for render: {error}"))?;
        let mut output = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut output);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let replacements = docx_replacements(input.chunks)?;

        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().to_string();
            writer
                .start_file(&name, options)
                .map_err(|error| error.to_string())?;
            if name == DOCUMENT_XML {
                let mut xml = String::new();
                file.read_to_string(&mut xml)
                    .map_err(|error| error.to_string())?;
                let rendered = replace_word_text_blocks(&xml, &replacements)?;
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
struct TextBlock {
    index: usize,
    in_table: bool,
    nodes: Vec<TextNode>,
}

impl TextBlock {
    fn text(&self) -> String {
        self.nodes
            .iter()
            .map(|node| node.text.as_str())
            .collect::<String>()
    }

    fn is_breakable(&self) -> bool {
        self.nodes.len() <= 1 && !self.in_table
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocxBlockMeta {
    block_index: usize,
    nodes: Vec<TextNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplacementText {
    Whole(String),
    Nodes(BTreeMap<usize, String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextBlockReplacement {
    block_index: usize,
    text: ReplacementText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocxUnitDescriptor {
    tag: String,
    block_index: usize,
}

fn validate_docx(path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("Unable to read DOCX: {error}"))?;
    docx_rs::read_docx(&bytes)
        .map(|_| ())
        .map_err(|error| format!("Unable to parse DOCX with docx-rs: {error}"))
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

fn extract_word_text_blocks(xml: &str) -> Result<Vec<TextBlock>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut blocks = Vec::new();
    let mut paragraph_depth = 0_usize;
    let mut paragraph_index = 0_usize;
    let mut paragraph_in_table = false;
    let mut table_depth = 0_usize;
    let mut text_index = 0_usize;
    let mut in_text = false;
    let mut nodes = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?
        {
            Event::Start(event) if local_name(event.name().as_ref()) == b"tbl" => {
                table_depth += 1;
            }
            Event::End(event) if local_name(event.name().as_ref()) == b"tbl" => {
                table_depth = table_depth.saturating_sub(1);
            }
            Event::Start(event) if local_name(event.name().as_ref()) == b"p" => {
                if paragraph_depth == 0 {
                    nodes.clear();
                    text_index = 0;
                    paragraph_in_table = table_depth > 0;
                }
                paragraph_depth += 1;
            }
            Event::End(event) if local_name(event.name().as_ref()) == b"p" => {
                if paragraph_depth > 0 {
                    paragraph_depth -= 1;
                    if paragraph_depth == 0 {
                        blocks.push(TextBlock {
                            index: paragraph_index,
                            in_table: paragraph_in_table,
                            nodes: nodes.clone(),
                        });
                        paragraph_index += 1;
                    }
                }
            }
            Event::Start(event)
                if paragraph_depth > 0 && local_name(event.name().as_ref()) == b"t" =>
            {
                in_text = true;
            }
            Event::End(event) if local_name(event.name().as_ref()) == b"t" => {
                in_text = false;
            }
            Event::Text(text) if paragraph_depth > 0 && in_text => {
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
    Ok(blocks)
}

fn docx_chunk_from_blocks(
    sequence: usize,
    blocks: Vec<ChunkedRawBlock<DocxBlockMeta>>,
) -> Result<ParsedChunk, String> {
    let mut source_parts = Vec::new();
    let mut preprocessed_parts = Vec::new();
    let mut entries = Vec::new();
    let mut next_run_index = 1_usize;

    for (local_index, block) in blocks.iter().enumerate() {
        let unit_id = format!("it{local_index}");
        let raw_text = block.text.clone();
        let (source_text, mut run_entries) =
            docx_source_for_block(block, &unit_id, &mut next_run_index);
        source_parts.push(format!("<{unit_id}>{source_text}</{unit_id}>"));
        preprocessed_parts.push(raw_text.clone());
        entries.push(PlaceholderEntry {
            id: unit_id,
            kind: DOCX_BLOCK_KIND.into(),
            original: raw_text,
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!("block:{}", block.metadata.block_index)),
        });
        entries.append(&mut run_entries);
    }

    let map = PlaceholderMap {
        version: super::types::PLACEHOLDER_MAP_VERSION,
        format: DocumentFormat::Docx,
        content_format: ContentFormat::Xml,
        block_ref: BlockRef {
            kind: DOCX_CHUNK_KIND.into(),
            path: Some(DOCUMENT_XML.into()),
            index: Some(sequence),
            pointer: Some(format!("blocks:{}", blocks.len())),
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

fn docx_source_for_block(
    block: &ChunkedRawBlock<DocxBlockMeta>,
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
            kind: DOCX_RUN_KIND.into(),
            original: node.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!("unit:{unit_id};text:{}", node.index)),
        });
    }
    (source, entries)
}

impl DocxBlockMeta {
    fn nodes_text_len(&self) -> usize {
        self.nodes.iter().map(|node| node.text.len()).sum()
    }
}

#[cfg(test)]
fn map_for_text_block(
    format: DocumentFormat,
    block_index: usize,
    nodes: &[TextNode],
) -> Result<PlaceholderMap, String> {
    let mut map = PlaceholderMap::empty(
        format,
        ContentFormat::Xml,
        BlockRef {
            kind: DOCX_BLOCK_KIND.into(),
            path: Some(DOCUMENT_XML.into()),
            index: Some(block_index),
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
                kind: DOCX_RUN_KIND.into(),
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

fn docx_replacements(
    chunks: &[super::types::RenderedChunk],
) -> Result<Vec<TextBlockReplacement>, String> {
    let mut collected = Vec::<(i64, usize, usize, ReplacementText)>::new();
    let mut order = 0_usize;
    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        match map.block_ref.kind.as_str() {
            DOCX_CHUNK_KIND if map.block_ref.path.as_deref() == Some(DOCUMENT_XML) => {
                let units = docx_unit_descriptors(&map)?;
                let tagged_text = if chunk.after_translate_text.contains("<it") {
                    chunk.after_translate_text.as_str()
                } else {
                    chunk.translated_text.as_str()
                };
                let translated_units = extract_docx_unit_text(tagged_text, &units)?;
                for (unit, translated_text) in units.into_iter().zip(translated_units) {
                    let run_entries = run_entries_for_unit(&map, &unit.tag);
                    let text = tagged_node_replacements_for_entries(&run_entries, &translated_text)
                        .map(ReplacementText::Nodes)
                        .unwrap_or_else(|| {
                            ReplacementText::Whole(strip_run_placeholders(&translated_text))
                        });
                    collected.push((chunk.sequence, order, unit.block_index, text));
                    order += 1;
                }
            }
            DOCX_BLOCK_KIND if map.block_ref.path.as_deref() == Some(DOCUMENT_XML) => {
                let Some(block_index) = map.block_ref.index else {
                    continue;
                };
                let text = tagged_node_replacements(&map, &chunk.after_translate_text)
                    .map(ReplacementText::Nodes)
                    .unwrap_or_else(|| {
                        ReplacementText::Whole(strip_run_placeholders(&chunk.translated_text))
                    });
                collected.push((chunk.sequence, order, block_index, text));
                order += 1;
            }
            _ => {}
        }
    }
    collected.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut merged = BTreeMap::<usize, ReplacementText>::new();
    for (_, _, block_index, text) in collected {
        match merged.get_mut(&block_index) {
            Some(existing) => merge_replacement_text(existing, text),
            None => {
                merged.insert(block_index, text);
            }
        }
    }
    Ok(merged
        .into_iter()
        .map(|(block_index, text)| TextBlockReplacement { block_index, text })
        .collect())
}

fn tagged_node_replacements(
    map: &PlaceholderMap,
    translated: &str,
) -> Option<BTreeMap<usize, String>> {
    let entries = map
        .entries
        .iter()
        .filter(|entry| entry.kind == DOCX_RUN_KIND)
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

fn restore_docx_text_chunk(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    let units = docx_unit_descriptors(map)?;
    let translated_units = extract_docx_unit_text(after_translate_text, &units)?;
    let mut restored = Vec::new();
    for (unit, text) in units.into_iter().zip(translated_units) {
        let unit_text = if run_entries_for_unit(map, &unit.tag).is_empty() {
            text
        } else {
            strip_run_placeholders(&text)
        };
        restored.push(format!("<{}>{}</{}>", unit.tag, unit_text, unit.tag));
    }
    Ok(restored.join("\n"))
}

fn docx_unit_descriptors(map: &PlaceholderMap) -> Result<Vec<DocxUnitDescriptor>, String> {
    let mut units = Vec::new();
    for entry in &map.entries {
        if entry.kind != DOCX_BLOCK_KIND {
            continue;
        }
        let block_index = entry
            .native_ref
            .as_deref()
            .and_then(|native_ref| native_ref.strip_prefix("block:"))
            .and_then(|index| index.parse::<usize>().ok())
            .ok_or_else(|| format!("DOCX unit {} is missing block reference", entry.id))?;
        units.push(DocxUnitDescriptor {
            tag: entry.id.clone(),
            block_index,
        });
    }
    if units.is_empty() {
        return Err("DOCX chunk has no text block entries".into());
    }
    Ok(units)
}

fn extract_docx_unit_text(text: &str, units: &[DocxUnitDescriptor]) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for unit in units {
        let value = extract_tagged_text(text, &unit.tag).ok_or_else(|| {
            format!(
                "Translated DOCX chunk is missing expected unit tag <{}>",
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
            entry.kind == DOCX_RUN_KIND
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

fn strip_run_placeholders(text: &str) -> String {
    Regex::new(r"(?is)</?\s*t\d+\s*>")
        .expect("static DOCX run placeholder regex")
        .replace_all(text, "")
        .to_string()
}

fn replace_word_text_blocks(
    xml: &str,
    replacements: &[TextBlockReplacement],
) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut paragraph_depth = 0_usize;
    let mut paragraph_index = 0_usize;
    let mut text_index = 0_usize;
    let mut in_text = false;
    let mut replacement = None::<&ReplacementText>;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?;
        match event {
            Event::Start(ref event) if local_name(event.name().as_ref()) == b"p" => {
                if paragraph_depth == 0 {
                    replacement = replacements
                        .iter()
                        .find(|item| item.block_index == paragraph_index)
                        .map(|item| &item.text);
                    text_index = 0;
                }
                paragraph_depth += 1;
                writer
                    .write_event(Event::Start(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::End(ref event) if local_name(event.name().as_ref()) == b"p" => {
                if paragraph_depth > 0 {
                    paragraph_depth -= 1;
                    if paragraph_depth == 0 {
                        paragraph_index += 1;
                        replacement = None;
                    }
                }
                writer
                    .write_event(Event::End(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::Start(ref event)
                if paragraph_depth > 0 && local_name(event.name().as_ref()) == b"t" =>
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
            Event::Text(ref text) if paragraph_depth > 0 && in_text => {
                if let Some(replacement_text) = replacement {
                    let value = replacement_value(replacement_text, text_index);
                    writer
                        .write_event(Event::Text(BytesText::new(&value)))
                        .map_err(|error| error.to_string())?;
                    text_index += 1;
                } else {
                    writer
                        .write_event(Event::Text(text.clone()))
                        .map_err(|error| error.to_string())?;
                }
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
    fn extracts_table_paragraphs_and_run_placeholders() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b /></w:rPr><w:t>World</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let blocks = extract_word_text_blocks(xml).expect("extract blocks");

        assert_eq!(blocks.len(), 2);
        assert_eq!(
            source_text_for_nodes(&blocks[0].nodes),
            "<t1>Hello </t1><t2>World</t2>"
        );
        assert_eq!(source_text_for_nodes(&blocks[1].nodes), "Cell");
    }

    #[test]
    fn replaces_run_text_without_removing_run_properties() {
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b /></w:rPr><w:t>World</w:t></w:r></w:p></w:body></w:document>"#;
        let replacement = TextBlockReplacement {
            block_index: 0,
            text: ReplacementText::Nodes(BTreeMap::from([
                (0, "Hola ".to_string()),
                (1, "Mundo".to_string()),
            ])),
        };

        let rendered = replace_word_text_blocks(xml, &[replacement]).expect("replace text");

        assert!(rendered.contains("<w:b"));
        assert!(rendered.contains(">Hola <"));
        assert!(rendered.contains(">Mundo<"));
    }

    #[test]
    fn render_preserves_non_document_entries() {
        let path = temp_path("docx-render.docx");
        write_test_docx(&path, r#"<w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b /></w:rPr><w:t>World</w:t></w:r></w:p>"#)
            .expect("write test docx");
        let chunks = vec![rendered_docx_chunk(
            0,
            "<t1>Hello </t1><t2>World</t2>",
            "<t1>Hola </t1><t2>Mundo</t2>",
            "Hola Mundo",
        )];

        let output = DocxParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &chunks,
            })
            .expect("render docx");
        let document_xml = read_zip_entry_from_bytes(&output, DOCUMENT_XML).expect("document xml");
        let styles_xml = read_zip_entry_from_bytes(&output, "word/styles.xml").expect("styles xml");

        assert!(document_xml.contains("<w:b"));
        assert!(document_xml.contains(">Hola <"));
        assert!(document_xml.contains(">Mundo<"));
        assert_eq!(styles_xml, "<w:styles />");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parse_generates_run_placeholders_after_chunking_only() {
        let path = temp_path("docx-parse.docx");
        write_test_docx(
            &path,
            r#"<w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b /></w:rPr><w:t>World</w:t></w:r></w:p>"#,
        )
        .expect("write test docx");

        let chunks = DocxParser
            .parse(ParserInput {
                source_path: &path,
                token_limit: 1,
            })
            .expect("parse docx");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].preprocessed_text, "Hello World");
        assert!(!chunks[0].preprocessed_text.contains("<t1>"));
        assert_eq!(
            chunks[0].source_text,
            "<it0><t1>Hello </t1><t2>World</t2></it0>"
        );
        let _ = std::fs::remove_file(path);
    }

    fn rendered_docx_chunk(
        index: usize,
        source_text: &str,
        after_translate_text: &str,
        translated_text: &str,
    ) -> RenderedChunk {
        let nodes = if source_text.contains("<t1>") {
            vec![
                TextNode {
                    index: 0,
                    text: "Hello ".into(),
                },
                TextNode {
                    index: 1,
                    text: "World".into(),
                },
            ]
        } else {
            vec![TextNode {
                index: 0,
                text: source_text.into(),
            }]
        };
        RenderedChunk {
            sequence: index as i64,
            source_text: source_text.into(),
            after_translate_text: after_translate_text.into(),
            translated_text: translated_text.into(),
            map_json: map_for_text_block(DocumentFormat::Docx, index, &nodes)
                .expect("map")
                .to_json()
                .expect("serialize map"),
        }
    }

    fn write_test_docx(path: &std::path::Path, body_xml: &str) -> Result<(), String> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body_xml}</w:body></w:document>"#
        );
        let entries = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            (
                "word/_rels/document.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
            ),
            ("word/document.xml", document.as_str()),
            ("word/styles.xml", "<w:styles />"),
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
