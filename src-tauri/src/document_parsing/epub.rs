use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lib_epub::builder::{EpubBuilder, EpubVersion3};
use lib_epub::epub::EpubDoc;
use lib_epub::types::ManifestItem;
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use regex::Regex;
use xml5ever::driver::{parse_document, XmlParseOpts};
use xml5ever::serialize::{serialize, SerializeOpts};
use xml5ever::tendril::{StrTendril, TendrilSink};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{
    BlockRef, ParsedChunk, ParserProgress, PlaceholderEntry, PlaceholderMap, RenderInput,
    RenderedChunk, PLACEHOLDER_MAP_VERSION,
};
use super::{
    chunk_raw_block_refs, chunk_raw_block_refs_with_progress, token_limit_usize, ChunkedRawBlock,
    DocumentParser, RawBlockRef,
};

const EPUB_DOM_CHUNK_KIND: &str = "epub-xhtml-dom-chunk";
const EPUB_TEXT_BLOCK_KIND: &str = "epub-xhtml-dom-text-block";
const EPUB_TEXT_NODE_KIND: &str = "epub-xhtml-dom-text-node";
const EPUB_ATTRIBUTE_KIND: &str = "epub-xhtml-dom-attribute";
const TRANSLATABLE_ATTRIBUTES: &[&str] = &["alt", "title", "placeholder"];

pub struct EpubParser;

impl DocumentParser for EpubParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let doc = EpubDoc::new(input.source_path)
            .map_err(|error| format!("Unable to open EPUB source: {error}"))?;
        let mut chunks = Vec::new();
        let mut progress = input.progress;
        for page in epub_page_refs(&doc) {
            let text = read_manifest_text(&doc, &page.id)?;
            let document = parse_xhtml_document(&text)?;
            let mut parsed = parse_epub_xhtml_page(
                &page.path,
                &document,
                input.token_limit,
                progress.as_deref_mut(),
            )?;
            chunks.append(&mut parsed);
        }
        resequence(&mut chunks);
        drop(doc);
        Ok(chunks)
    }

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        let map = super::parse_map(map_json)?;
        match map.block_ref.kind.as_str() {
            EPUB_DOM_CHUNK_KIND => restore_epub_dom_chunk(&map, after_translate_text),
            EPUB_ATTRIBUTE_KIND => Ok(after_translate_text.to_string()),
            _ => Err(format!(
                "Unsupported EPUB placeholder map kind `{}`",
                map.block_ref.kind
            )),
        }
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let mut doc = EpubDoc::new(input.source_path)
            .map_err(|error| format!("Unable to read EPUB for render: {error}"))?;
        let pages = epub_page_refs(&doc);
        let mut builder = EpubBuilder::<EpubVersion3>::from(&mut doc)
            .map_err(|error| format!("Unable to initialize EPUB builder: {error}"))?;
        let temp_dir = unique_temp_dir("insitu-epub-render")?;

        for page in pages {
            let source_text = read_manifest_text(&doc, &page.id)?;
            let rendered = render_epub_xhtml_page(&page.path, &source_text, input.chunks)?;
            let local_path = write_temp_resource(&temp_dir, &page.path, rendered.as_bytes())?;
            let manifest = doc
                .manifest
                .get(&page.id)
                .cloned()
                .ok_or_else(|| format!("EPUB manifest item `{}` disappeared", page.id))?;
            builder
                .add_manifest(local_path.to_string_lossy().to_string(), manifest)
                .map_err(|error| format!("Unable to add translated EPUB page: {error}"))?;
        }

        let output_path = temp_dir.join("translated.epub");
        builder
            .make(&output_path)
            .map_err(|error| format!("Unable to build translated EPUB: {error}"))?;
        let bytes = std::fs::read(&output_path)
            .map_err(|error| format!("Unable to read translated EPUB output: {error}"))?;
        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubPageRef {
    id: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubTextMeta {
    path: String,
    node_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubTextBlock {
    node_index: usize,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubUnitDescriptor {
    tag: String,
    path: String,
    node_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpubAttributeReplacement {
    node_index: usize,
    attr_name: String,
    text: String,
}

fn epub_page_refs<R: Read + Seek + Send>(doc: &EpubDoc<R>) -> Vec<EpubPageRef> {
    let mut seen = HashSet::new();
    let mut pages = Vec::new();

    for spine_item in &doc.spine {
        if let Some(item) = doc.manifest.get(&spine_item.idref) {
            if is_html_manifest_item(item) && seen.insert(item.id.clone()) {
                pages.push(EpubPageRef {
                    id: item.id.clone(),
                    path: normalize_zip_path(&item.path.to_string_lossy()),
                });
            }
        }
    }

    for item in doc.manifest.values() {
        if is_html_manifest_item(item) && seen.insert(item.id.clone()) {
            pages.push(EpubPageRef {
                id: item.id.clone(),
                path: normalize_zip_path(&item.path.to_string_lossy()),
            });
        }
    }

    pages
}

fn is_html_manifest_item(item: &ManifestItem) -> bool {
    let path = item.path.to_string_lossy().to_ascii_lowercase();
    let mime = item.mime.to_ascii_lowercase();
    path.ends_with(".xhtml")
        || path.ends_with(".html")
        || path.ends_with(".htm")
        || mime == "application/xhtml+xml"
        || mime == "text/html"
}

fn read_manifest_text<R: Read + Seek + Send>(
    doc: &EpubDoc<R>,
    manifest_id: &str,
) -> Result<String, String> {
    let (bytes, _) = doc
        .get_manifest_item(manifest_id)
        .map_err(|error| format!("Unable to read EPUB manifest item `{manifest_id}`: {error}"))?;
    String::from_utf8(bytes)
        .map_err(|error| format!("EPUB XHTML item `{manifest_id}` is not valid UTF-8: {error}"))
}

fn parse_epub_xhtml_page(
    path: &str,
    document: &RcDom,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let mut chunks = epub_text_chunks(path, document, token_limit, progress)?;
    let attributes = epub_attribute_chunks(path, document, chunks.len())?;
    chunks.extend(attributes);
    Ok(chunks)
}

fn epub_text_chunks(
    path: &str,
    document: &RcDom,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let raw_blocks = collect_epub_text_blocks(document)
        .into_iter()
        .map(|block| {
            RawBlockRef::new(
                block.text,
                true,
                EpubTextMeta {
                    path: path.to_string(),
                    node_index: block.node_index,
                },
            )
        })
        .collect::<Vec<_>>();

    if raw_blocks.is_empty() {
        return Ok(Vec::new());
    }

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
        .filter(|(_, blocks)| !blocks.is_empty())
        .map(|(sequence, blocks)| epub_chunk_from_blocks(path, sequence, blocks))
        .collect()
}

fn collect_epub_text_blocks(document: &RcDom) -> Vec<EpubTextBlock> {
    let mut blocks = Vec::new();
    let mut node_index = 0_usize;
    collect_epub_text_blocks_inner(
        &document.document,
        &mut Vec::new(),
        &mut node_index,
        &mut blocks,
    );
    blocks
}

fn collect_epub_text_blocks_inner(
    node: &Handle,
    ancestor_names: &mut Vec<String>,
    node_index: &mut usize,
    blocks: &mut Vec<EpubTextBlock>,
) {
    let current_index = *node_index;
    *node_index += 1;

    match &node.data {
        NodeData::Text { contents } => {
            if is_translatable_name_context(None, ancestor_names) {
                let text = contents.borrow().to_string();
                if let Some(core) = trimmed_core(&text) {
                    blocks.push(EpubTextBlock {
                        node_index: current_index,
                        text: core.to_string(),
                    });
                }
            }
        }
        NodeData::Element { name, .. } => {
            ancestor_names.push(name.local.to_string());
            let children = node.children.borrow().clone();
            for child in children {
                collect_epub_text_blocks_inner(&child, ancestor_names, node_index, blocks);
            }
            ancestor_names.pop();
            return;
        }
        _ => {}
    }

    let children = node.children.borrow().clone();
    for child in children {
        collect_epub_text_blocks_inner(&child, ancestor_names, node_index, blocks);
    }
}

fn epub_chunk_from_blocks(
    path: &str,
    sequence: usize,
    blocks: Vec<ChunkedRawBlock<EpubTextMeta>>,
) -> Result<ParsedChunk, String> {
    let mut source_parts = Vec::new();
    let mut preprocessed_parts = Vec::new();
    let mut entries = Vec::new();
    let mut next_text_index = 1_usize;

    for (local_index, block) in blocks.iter().enumerate() {
        let unit_id = format!("it{local_index}");
        let text_id = format!("t{next_text_index}");
        next_text_index += 1;

        source_parts.push(format!(
            "<{unit_id}><{text_id}>{}</{text_id}></{unit_id}>",
            block.text
        ));
        preprocessed_parts.push(block.text.clone());
        entries.push(PlaceholderEntry {
            id: unit_id.clone(),
            kind: EPUB_TEXT_BLOCK_KIND.into(),
            original: block.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!(
                "path:{};node:{};range:{}:{}",
                block.metadata.path,
                block.metadata.node_index,
                block.source_start,
                block.source_end
            )),
        });
        entries.push(PlaceholderEntry {
            id: text_id,
            kind: EPUB_TEXT_NODE_KIND.into(),
            original: block.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!(
                "unit:{unit_id};path:{};node:{}",
                block.metadata.path, block.metadata.node_index
            )),
        });
    }

    let map = PlaceholderMap {
        version: PLACEHOLDER_MAP_VERSION,
        format: DocumentFormat::Epub,
        content_format: ContentFormat::Xhtml,
        block_ref: BlockRef {
            kind: EPUB_DOM_CHUNK_KIND.into(),
            path: Some(path.to_string()),
            index: Some(sequence),
            pointer: Some(format!("units:{}", blocks.len())),
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

fn epub_attribute_chunks(
    path: &str,
    document: &RcDom,
    start_sequence: usize,
) -> Result<Vec<ParsedChunk>, String> {
    let mut chunks = Vec::new();
    let mut node_index = 0_usize;
    collect_epub_attribute_chunks_inner(
        path,
        &document.document,
        &mut Vec::new(),
        &mut node_index,
        start_sequence,
        &mut chunks,
    )?;
    Ok(chunks)
}

fn collect_epub_attribute_chunks_inner(
    path: &str,
    node: &Handle,
    ancestor_names: &mut Vec<String>,
    node_index: &mut usize,
    start_sequence: usize,
    chunks: &mut Vec<ParsedChunk>,
) -> Result<(), String> {
    let current_index = *node_index;
    *node_index += 1;

    if let NodeData::Element { name, attrs, .. } = &node.data {
        let element_name = name.local.to_string();
        if is_translatable_name_context(Some(&element_name), ancestor_names) {
            for attr_name in TRANSLATABLE_ATTRIBUTES {
                let attrs = attrs.borrow();
                let Some(attribute) = attrs
                    .iter()
                    .find(|attribute| attribute.name.local.as_ref() == *attr_name)
                else {
                    continue;
                };
                let original = attribute.value.to_string();
                let Some((_, core, _)) = split_core(&original) else {
                    continue;
                };
                let sequence = start_sequence + chunks.len();
                let map = PlaceholderMap::empty(
                    DocumentFormat::Epub,
                    ContentFormat::Xhtml,
                    BlockRef {
                        kind: EPUB_ATTRIBUTE_KIND.into(),
                        path: Some(path.to_string()),
                        index: Some(sequence),
                        pointer: Some(format!("path:{path};node:{current_index};attr:{attr_name}")),
                        prefix: String::new(),
                        suffix: String::new(),
                    },
                );
                chunks.push(ParsedChunk {
                    sequence: sequence as i64,
                    preprocessed_text: core.to_string(),
                    source_text: core.to_string(),
                    map_json: map.to_json()?,
                });
            }
        }

        ancestor_names.push(element_name);
        let children = node.children.borrow().clone();
        for child in children {
            collect_epub_attribute_chunks_inner(
                path,
                &child,
                ancestor_names,
                node_index,
                start_sequence,
                chunks,
            )?;
        }
        ancestor_names.pop();
        return Ok(());
    }

    let children = node.children.borrow().clone();
    for child in children {
        collect_epub_attribute_chunks_inner(
            path,
            &child,
            ancestor_names,
            node_index,
            start_sequence,
            chunks,
        )?;
    }
    Ok(())
}

fn restore_epub_dom_chunk(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    let units = epub_unit_descriptors(map)?;
    let mut restored = Vec::new();
    for unit in units {
        let unit_text = extract_tagged_text(after_translate_text, &unit.tag).ok_or_else(|| {
            format!(
                "Translated EPUB XHTML chunk is missing expected unit tag <{}>",
                unit.tag
            )
        })?;
        for entry in text_entries_for_unit(map, &unit.tag) {
            extract_tagged_text(&unit_text, &entry.id).ok_or_else(|| {
                format!(
                    "Translated EPUB XHTML unit <{}> is missing expected text node tag <{}>",
                    unit.tag, entry.id
                )
            })?;
        }
        restored.push(format!(
            "<{}>{}</{}>",
            unit.tag,
            strip_text_placeholders(&unit_text),
            unit.tag
        ));
    }
    Ok(restored.join("\n"))
}

fn render_epub_xhtml_page(
    path: &str,
    original_text: &str,
    chunks: &[RenderedChunk],
) -> Result<String, String> {
    let document = parse_xhtml_document(original_text)?;
    let text_replacements = epub_text_replacements(path, chunks)?;
    let attribute_replacements = epub_attribute_replacements(path, chunks)?;

    apply_text_replacements(&document, text_replacements)?;
    apply_attribute_replacements(&document, attribute_replacements)?;
    serialize_xhtml_document(&document)
}

fn epub_text_replacements(
    path: &str,
    chunks: &[RenderedChunk],
) -> Result<BTreeMap<usize, String>, String> {
    let mut collected = Vec::<(i64, usize, usize, String)>::new();
    let mut order = 0_usize;

    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind != EPUB_DOM_CHUNK_KIND || !path_matches(&map, path) {
            continue;
        }
        let units = epub_unit_descriptors(&map)?;
        for unit in units {
            let unit_text = extract_tagged_text(&chunk.after_translate_text, &unit.tag)
                .ok_or_else(|| {
                    format!(
                        "Translated EPUB XHTML chunk is missing expected unit tag <{}>",
                        unit.tag
                    )
                })?;
            let entries = text_entries_for_unit(&map, &unit.tag);
            if entries.is_empty() {
                collected.push((
                    chunk.sequence,
                    order,
                    unit.node_index,
                    strip_text_placeholders(&unit_text),
                ));
                order += 1;
                continue;
            }
            for entry in entries {
                let node_index =
                    node_index_from_native_ref(entry.native_ref.as_deref().unwrap_or(""))
                        .unwrap_or(unit.node_index);
                let text = extract_tagged_text(&unit_text, &entry.id).ok_or_else(|| {
                    format!(
                        "Translated EPUB XHTML unit <{}> is missing expected text node tag <{}>",
                        unit.tag, entry.id
                    )
                })?;
                collected.push((chunk.sequence, order, node_index, text));
                order += 1;
            }
        }
    }

    collected.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut replacements = BTreeMap::<usize, String>::new();
    for (_, _, node_index, text) in collected {
        replacements
            .entry(node_index)
            .and_modify(|existing| existing.push_str(&text))
            .or_insert(text);
    }
    Ok(replacements)
}

fn epub_attribute_replacements(
    path: &str,
    chunks: &[RenderedChunk],
) -> Result<Vec<EpubAttributeReplacement>, String> {
    let mut replacements = Vec::new();
    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind != EPUB_ATTRIBUTE_KIND || !path_matches(&map, path) {
            continue;
        }
        let Some(pointer) = map.block_ref.pointer.as_deref() else {
            return Err("EPUB XHTML attribute chunk is missing pointer".into());
        };
        let Some((node_index, attr_name)) = parse_attribute_pointer(pointer) else {
            return Err(format!("Invalid EPUB XHTML attribute pointer `{pointer}`"));
        };
        replacements.push(EpubAttributeReplacement {
            node_index,
            attr_name,
            text: chunk.translated_text.clone(),
        });
    }
    Ok(replacements)
}

fn apply_text_replacements(
    document: &RcDom,
    replacements: BTreeMap<usize, String>,
) -> Result<(), String> {
    if replacements.is_empty() {
        return Ok(());
    }
    let node_handles = collect_node_handles(&document.document);
    for (node_index, replacement) in replacements {
        let Some(node) = node_handles.get(node_index) else {
            return Err(format!(
                "EPUB XHTML text node index {node_index} is out of range"
            ));
        };
        let NodeData::Text { contents } = &node.data else {
            return Err(format!("EPUB XHTML node {node_index} is not a text node"));
        };
        let original = contents.borrow().to_string();
        let (prefix, _, suffix) = split_core(&original).unwrap_or(("", "", ""));
        *contents.borrow_mut() = StrTendril::from_slice(&format!("{prefix}{replacement}{suffix}"));
    }
    Ok(())
}

fn apply_attribute_replacements(
    document: &RcDom,
    replacements: Vec<EpubAttributeReplacement>,
) -> Result<(), String> {
    if replacements.is_empty() {
        return Ok(());
    }
    let node_handles = collect_node_handles(&document.document);
    for replacement in replacements {
        let Some(node) = node_handles.get(replacement.node_index) else {
            return Err(format!(
                "EPUB XHTML attribute node index {} is out of range",
                replacement.node_index
            ));
        };
        let NodeData::Element { attrs, .. } = &node.data else {
            return Err(format!(
                "EPUB XHTML node {} is not an element node",
                replacement.node_index
            ));
        };
        let mut attrs = attrs.borrow_mut();
        let Some(attribute) = attrs
            .iter_mut()
            .find(|attribute| attribute.name.local.as_ref() == replacement.attr_name.as_str())
        else {
            return Err(format!(
                "EPUB XHTML element node {} is missing `{}` attribute",
                replacement.node_index, replacement.attr_name
            ));
        };
        let original = attribute.value.to_string();
        let (prefix, _, suffix) = split_core(&original).unwrap_or(("", "", ""));
        attribute.value = StrTendril::from_slice(&format!("{prefix}{}{suffix}", replacement.text));
    }
    Ok(())
}

fn collect_node_handles(root: &Handle) -> Vec<Handle> {
    let mut handles = Vec::new();
    collect_node_handles_inner(root, &mut handles);
    handles
}

fn collect_node_handles_inner(node: &Handle, handles: &mut Vec<Handle>) {
    handles.push(node.clone());
    let children = node.children.borrow().clone();
    for child in children {
        collect_node_handles_inner(&child, handles);
    }
}

fn epub_unit_descriptors(map: &PlaceholderMap) -> Result<Vec<EpubUnitDescriptor>, String> {
    let units = map
        .entries
        .iter()
        .filter(|entry| entry.kind == EPUB_TEXT_BLOCK_KIND)
        .map(|entry| {
            let native_ref = entry.native_ref.as_deref().unwrap_or("");
            let path = path_from_native_ref(native_ref)
                .ok_or_else(|| format!("EPUB XHTML unit {} is missing path reference", entry.id))?;
            let node_index = node_index_from_native_ref(native_ref)
                .ok_or_else(|| format!("EPUB XHTML unit {} is missing node reference", entry.id))?;
            Ok(EpubUnitDescriptor {
                tag: entry.id.clone(),
                path,
                node_index,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if units.is_empty() {
        return Err("EPUB XHTML DOM chunk has no text block entries".into());
    }
    Ok(units)
}

fn text_entries_for_unit<'a>(map: &'a PlaceholderMap, unit_tag: &str) -> Vec<&'a PlaceholderEntry> {
    map.entries
        .iter()
        .filter(|entry| {
            entry.kind == EPUB_TEXT_NODE_KIND
                && entry
                    .native_ref
                    .as_deref()
                    .is_some_and(|native_ref| native_ref.starts_with(&format!("unit:{unit_tag};")))
        })
        .collect()
}

fn parse_xhtml_document(text: &str) -> Result<RcDom, String> {
    let document = parse_document(RcDom::default(), XmlParseOpts::default()).one(text);
    let errors = document.errors.borrow();
    if !errors.is_empty() {
        return Err(format!("Unable to parse EPUB XHTML: {}", errors.join("; ")));
    }
    drop(errors);
    Ok(document)
}

fn serialize_xhtml_document(document: &RcDom) -> Result<String, String> {
    let mut bytes = Vec::new();
    serialize(
        &mut bytes,
        &SerializableHandle::from(document.document.clone()),
        SerializeOpts::default(),
    )
    .map_err(|error| format!("Unable to serialize EPUB XHTML: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|error| format!("Serialized EPUB XHTML is not valid UTF-8: {error}"))?;
    Ok(collapse_empty_xhtml_elements(&text))
}

fn collapse_empty_xhtml_elements(text: &str) -> String {
    let mut collapsed = text.to_string();
    for tag in [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "span", "track", "wbr",
    ] {
        let pattern = Regex::new(&format!(r"(?i)<{}([^>]*)></{}>", tag, tag))
            .expect("static XHTML empty element collapse regex");
        collapsed = pattern
            .replace_all(&collapsed, format!("<{tag}$1 />"))
            .to_string();
    }
    collapsed
}

fn is_translatable_name_context(self_name: Option<&str>, ancestor_names: &[String]) -> bool {
    let mut in_head = false;
    let mut in_title = false;

    if self_name.is_some_and(is_blocked_element_name) {
        return false;
    }

    for name in ancestor_names {
        match name.as_str() {
            "head" => in_head = true,
            "title" => in_title = true,
            name if is_blocked_element_name(name) => return false,
            _ => {}
        }
    }

    !(in_head && !in_title)
}

fn is_blocked_element_name(name: &str) -> bool {
    matches!(name, "script" | "style" | "link" | "meta" | "template")
}

fn resequence(chunks: &mut [ParsedChunk]) {
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
    }
}

fn path_matches(map: &PlaceholderMap, target_path: &str) -> bool {
    map.block_ref
        .path
        .as_deref()
        .map(normalize_zip_path)
        .is_some_and(|path| path == normalize_zip_path(target_path))
}

fn path_from_native_ref(native_ref: &str) -> Option<String> {
    native_ref
        .split(';')
        .find_map(|part| part.strip_prefix("path:"))
        .map(normalize_zip_path)
}

fn node_index_from_native_ref(native_ref: &str) -> Option<usize> {
    native_ref
        .split(';')
        .find_map(|part| part.strip_prefix("node:"))
        .and_then(|index| index.parse::<usize>().ok())
}

fn parse_attribute_pointer(pointer: &str) -> Option<(usize, String)> {
    let (_, rest) = pointer.split_once(";node:")?;
    let (node, attr) = rest.split_once(";attr:")?;
    let node_index = node.parse::<usize>().ok()?;
    Some((node_index, attr.to_string()))
}

fn extract_tagged_text(text: &str, id: &str) -> Option<String> {
    let pattern = Regex::new(&format!(
        r"(?s)<{}>(.*?)</{}>",
        regex::escape(id),
        regex::escape(id)
    ))
    .ok()?;
    pattern
        .captures(text)
        .and_then(|captures| captures.get(1).map(|value| value.as_str().to_string()))
}

fn strip_text_placeholders(text: &str) -> String {
    let pattern = Regex::new(r"</?t\d+>").expect("static EPUB XHTML text placeholder strip regex");
    pattern.replace_all(text, "").to_string()
}

fn trimmed_core(text: &str) -> Option<&str> {
    let (_, core, _) = split_core(text)?;
    Some(core)
}

fn split_core(text: &str) -> Option<(&str, &str, &str)> {
    if text.trim().is_empty() {
        return None;
    }
    let core_start = leading_whitespace_len(text);
    let core_end = text.len() - trailing_whitespace_len(text);
    if core_start >= core_end {
        return None;
    }
    Some((
        &text[..core_start],
        &text[core_start..core_end],
        &text[core_end..],
    ))
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

fn normalize_zip_path(path: impl AsRef<str>) -> String {
    path.as_ref()
        .replace('\\', "/")
        .trim_start_matches('/')
        .into()
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX_EPOCH: {error}"))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path)
        .map_err(|error| format!("Unable to create temporary EPUB directory: {error}"))?;
    Ok(path)
}

fn write_temp_resource(
    base_dir: &Path,
    resource_path: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let path = base_dir.join(Path::new(resource_path));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("Unable to create temporary EPUB resource directory: {error}")
        })?;
    }
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Unable to write temporary EPUB resource: {error}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use lib_epub::types::{MetadataItem, NavPoint, SpineItem};

    use super::*;

    #[test]
    fn parse_uses_spine_order_then_manifest_html_entries() {
        let path = unique_temp_epub_path("parse-order");
        write_test_epub(
            &path,
            &[
                (
                    "OEBPS/chapter2.xhtml",
                    "<html><body><p>Second</p></body></html>",
                    true,
                ),
                (
                    "OEBPS/chapter1.xhtml",
                    "<html><body><p>First</p></body></html>",
                    true,
                ),
                (
                    "OEBPS/appendix.xhtml",
                    "<html><body><p>Appendix</p></body></html>",
                    false,
                ),
            ],
            &[],
        )
        .expect("write epub");

        let chunks = EpubParser
            .parse(super::super::types::ParserInput {
                source_path: &path,
                token_limit: 800,
                progress: None,
            })
            .expect("parse epub");
        let sources = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(sources.find("Second").unwrap() < sources.find("First").unwrap());
        assert!(sources.find("First").unwrap() < sources.find("Appendix").unwrap());
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.sequence)
                .collect::<Vec<_>>(),
            (0..chunks.len() as i64).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn filters_non_visible_xhtml_and_extracts_attributes() {
        let xhtml = concat!(
            "<html><head>",
            "<title>Visible title</title>",
            "<meta name=\"description\" content=\"Skip meta\" />",
            "<link title=\"Skip link\" href=\"x.css\" />",
            "<style>.x{content:'Skip style'}</style>",
            "<script>const text = 'Skip script';</script>",
            "</head><body>",
            "<p title=\"Paragraph title\">Body text</p>",
            "<img alt=\"Cover\" />",
            "<template>Skip template</template>",
            "</body></html>"
        );
        let document = parse_xhtml_document(xhtml).expect("parse xhtml");
        let chunks =
            parse_epub_xhtml_page("OEBPS/chapter.xhtml", &document, 800, None).expect("parse page");
        let sources = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<Vec<_>>();

        assert!(sources
            .iter()
            .any(|source| source.contains("Visible title")));
        assert!(sources.iter().any(|source| source.contains("Body text")));
        assert!(sources.iter().any(|source| *source == "Paragraph title"));
        assert!(sources.iter().any(|source| *source == "Cover"));
        assert!(!sources.iter().any(|source| source.contains("Skip meta")));
        assert!(!sources.iter().any(|source| source.contains("Skip link")));
        assert!(!sources.iter().any(|source| source.contains("Skip style")));
        assert!(!sources.iter().any(|source| source.contains("Skip script")));
        assert!(!sources
            .iter()
            .any(|source| source.contains("Skip template")));
    }

    #[test]
    fn render_rewrites_xhtml_dom_and_keeps_assets() {
        let path = unique_temp_epub_path("render-dom");
        write_test_epub(
            &path,
            &[(
                "OEBPS/chapter.xhtml",
                "<html><body><p title=\" Greeting \">Hello <span />World<br /></p><img alt=\" Cover \" src=\"image.png\" /></body></html>",
                true,
            )],
            &[("OEBPS/image.png", tiny_png())],
        )
        .expect("write epub");
        let parsed = EpubParser
            .parse(super::super::types::ParserInput {
                source_path: &path,
                token_limit: 800,
                progress: None,
            })
            .expect("parse epub");
        let rendered_chunks = parsed
            .iter()
            .map(|chunk| {
                let replacement = chunk
                    .source_text
                    .replace("Hello", "Hola")
                    .replace("World", "Mundo")
                    .replace("Greeting", "Saludo")
                    .replace("Cover", "Portada");
                RenderedChunk {
                    sequence: chunk.sequence,
                    source_text: chunk.source_text.clone(),
                    after_translate_text: replacement.clone(),
                    translated_text: EpubParser
                        .restore_chunk(&chunk.map_json, &replacement)
                        .expect("restore chunk"),
                    map_json: chunk.map_json.clone(),
                }
            })
            .collect::<Vec<_>>();

        let output = EpubParser
            .render_document(RenderInput {
                source_path: &path,
                chunks: &rendered_chunks,
            })
            .expect("render epub");
        let rendered_path = write_output_epub("rendered-dom", &output).expect("write output");
        let doc = EpubDoc::new(&rendered_path).expect("read rendered epub");
        let (page, _) = doc
            .get_manifest_item_by_path("OEBPS/chapter.xhtml")
            .expect("read page");
        let page = String::from_utf8(page).expect("page utf8");
        let (image, _) = doc
            .get_manifest_item_by_path("OEBPS/image.png")
            .expect("read image");

        assert!(page.contains("Hola"));
        assert!(page.contains("Mundo"));
        assert!(page.contains("title=\" Saludo \""));
        assert!(page.contains("alt=\" Portada \""));
        assert!(page.contains("<span />"));
        assert!(page.contains("<br />"));
        assert!(page.contains("<img alt=\" Portada \" src=\"image.png\" />"));
        assert_eq!(image, tiny_png());
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(rendered_path);
    }

    fn write_test_epub(
        path: &Path,
        pages: &[(&str, &str, bool)],
        assets: &[(&str, &[u8])],
    ) -> Result<(), String> {
        let source_dir = unique_temp_dir("insitu-epub-test-src")?;
        let mut builder = EpubBuilder::<EpubVersion3>::new().map_err(|error| error.to_string())?;
        builder
            .add_rootfile("content.opf")
            .map_err(|error| error.to_string())?;
        builder
            .add_metadata(MetadataItem::new("title", "Test Book"))
            .add_metadata(MetadataItem::new("language", "en"));
        let mut identifier = MetadataItem::new("identifier", "test-book-id");
        identifier.with_id("pub-id");
        builder.add_metadata(identifier.build());

        for (index, (target_path, text, in_spine)) in pages.iter().enumerate() {
            let local_path = write_temp_resource(&source_dir, target_path, text.as_bytes())?;
            let id = format!("page-{index}");
            builder
                .add_manifest(
                    local_path.to_string_lossy().to_string(),
                    ManifestItem::new(&id, target_path).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            if *in_spine {
                builder.add_spine(SpineItem::new(&id));
            }
        }

        if let Some((target_path, _, _)) = pages.first() {
            let mut nav = NavPoint::new("Start");
            nav.with_content(target_path);
            builder.add_catalog_item(nav.build());
        }

        for (index, (target_path, bytes)) in assets.iter().enumerate() {
            let local_path = write_temp_resource(&source_dir, target_path, bytes)?;
            builder
                .add_manifest(
                    local_path.to_string_lossy().to_string(),
                    ManifestItem::new(&format!("asset-{index}"), target_path)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }

        builder.make(path).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_dir_all(source_dir);
        Ok(())
    }

    fn write_output_epub(prefix: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let path = unique_temp_epub_path(prefix);
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        Ok(path)
    }

    fn unique_temp_epub_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "insitu-{prefix}-{}-{nanos}.epub",
            std::process::id()
        ))
    }

    fn tiny_png() -> &'static [u8] {
        &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }
}
