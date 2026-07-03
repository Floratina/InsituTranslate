use std::collections::BTreeMap;

use regex::Regex;
use scraper::{Html, Node, StrTendril};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{
    BlockRef, ParsedChunk, ParserProgress, PlaceholderEntry, PlaceholderMap, RenderInput,
    RenderedChunk, PLACEHOLDER_MAP_VERSION,
};
use super::{
    chunk_raw_block_refs, chunk_raw_block_refs_with_progress, token_limit_usize, ChunkedRawBlock,
    DocumentParser, RawBlockRef,
};

const HTML_DOM_CHUNK_KIND: &str = "html-dom-chunk";
const HTML_TEXT_BLOCK_KIND: &str = "html-dom-text-block";
const HTML_TEXT_NODE_KIND: &str = "html-dom-text-node";
const HTML_ATTRIBUTE_KIND: &str = "html-dom-attribute";
const TRANSLATABLE_ATTRIBUTES: &[&str] = &["alt", "title", "placeholder"];

pub struct HtmlParser;

impl DocumentParser for HtmlParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read HTML source: {error}"))?;
        match input.progress {
            Some(progress) => {
                parse_html_text_with_progress(&text, input.token_limit, Some(progress))
            }
            None => parse_html_text(&text, input.token_limit),
        }
    }

    fn restore_chunk(&self, map_json: &str, after_translate_text: &str) -> Result<String, String> {
        let map = super::parse_map(map_json)?;
        if map.block_ref.kind == HTML_DOM_CHUNK_KIND {
            return restore_html_dom_chunk(&map, after_translate_text);
        }
        super::placeholders::restore_from_json(map_json, after_translate_text)
    }

    fn render_document(&self, input: RenderInput<'_>) -> Result<Vec<u8>, String> {
        let text = std::fs::read_to_string(input.source_path)
            .map_err(|error| format!("Unable to read HTML for render: {error}"))?;
        render_html_document(&text, input.chunks).map(|text| text.into_bytes())
    }
}

fn parse_html_text(text: &str, token_limit: i64) -> Result<Vec<ParsedChunk>, String> {
    parse_html_text_with_progress(text, token_limit, None)
}

fn parse_html_text_with_progress(
    text: &str,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let document = Html::parse_document(text);
    let mut chunks = html_text_chunks(&document, token_limit, progress)?;
    let attributes = html_attribute_chunks(&document, chunks.len())?;
    chunks.extend(attributes);
    Ok(chunks)
}

fn render_html_document(original_text: &str, chunks: &[RenderedChunk]) -> Result<String, String> {
    let mut document = Html::parse_document(original_text);
    let text_replacements = html_text_replacements(chunks)?;
    let attribute_replacements = html_attribute_replacements(chunks)?;

    apply_text_replacements(&mut document, text_replacements)?;
    apply_attribute_replacements(&mut document, attribute_replacements)?;

    Ok(document.html())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlTextMeta {
    node_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlUnitDescriptor {
    tag: String,
    node_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlAttributeReplacement {
    node_index: usize,
    attr_name: String,
    text: String,
}

fn html_text_chunks(
    document: &Html,
    token_limit: i64,
    progress: Option<&mut (dyn FnMut(ParserProgress) + Send + '_)>,
) -> Result<Vec<ParsedChunk>, String> {
    let raw_blocks = collect_html_text_blocks(document)
        .into_iter()
        .map(|block| {
            RawBlockRef::new(
                block.text,
                true,
                HtmlTextMeta {
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
        .map(|(sequence, blocks)| html_chunk_from_blocks(sequence, blocks))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlTextBlock {
    node_index: usize,
    text: String,
}

fn collect_html_text_blocks(document: &Html) -> Vec<HtmlTextBlock> {
    document
        .tree
        .nodes()
        .enumerate()
        .filter_map(|(node_index, node)| {
            let ancestor_names = node
                .ancestors()
                .filter_map(|ancestor| {
                    ancestor
                        .value()
                        .as_element()
                        .map(|element| element.name().to_string())
                })
                .collect::<Vec<_>>();
            if !is_translatable_name_context(None, &ancestor_names) {
                return None;
            }
            let Node::Text(text) = node.value() else {
                return None;
            };
            let core = trimmed_core(&text.text)?;
            Some(HtmlTextBlock {
                node_index,
                text: core.to_string(),
            })
        })
        .collect()
}

fn html_chunk_from_blocks(
    sequence: usize,
    blocks: Vec<ChunkedRawBlock<HtmlTextMeta>>,
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
            kind: HTML_TEXT_BLOCK_KIND.into(),
            original: block.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!(
                "node:{};range:{}:{}",
                block.metadata.node_index, block.source_start, block.source_end
            )),
        });
        entries.push(PlaceholderEntry {
            id: text_id,
            kind: HTML_TEXT_NODE_KIND.into(),
            original: block.text.clone(),
            open: String::new(),
            close: String::new(),
            translatable: true,
            native_ref: Some(format!("unit:{unit_id};node:{}", block.metadata.node_index)),
        });
    }

    let map = PlaceholderMap {
        version: PLACEHOLDER_MAP_VERSION,
        format: DocumentFormat::Html,
        content_format: ContentFormat::Html,
        block_ref: BlockRef {
            kind: HTML_DOM_CHUNK_KIND.into(),
            path: None,
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

fn html_attribute_chunks(
    document: &Html,
    start_sequence: usize,
) -> Result<Vec<ParsedChunk>, String> {
    let mut chunks = Vec::new();
    for (node_index, node) in document.tree.nodes().enumerate() {
        let Some(element) = node.value().as_element() else {
            continue;
        };
        let ancestor_names = node
            .ancestors()
            .filter_map(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .map(|element| element.name().to_string())
            })
            .collect::<Vec<_>>();
        if !is_translatable_name_context(Some(element.name()), &ancestor_names) {
            continue;
        }
        for attr_name in TRANSLATABLE_ATTRIBUTES {
            let Some(value) = element.attr(attr_name) else {
                continue;
            };
            let Some((_, core, _)) = split_core(value) else {
                continue;
            };
            let map = PlaceholderMap::empty(
                DocumentFormat::Html,
                ContentFormat::Html,
                BlockRef {
                    kind: HTML_ATTRIBUTE_KIND.into(),
                    path: None,
                    index: Some(start_sequence + chunks.len()),
                    pointer: Some(format!("node:{node_index};attr:{attr_name}")),
                    prefix: String::new(),
                    suffix: String::new(),
                },
            );
            chunks.push(ParsedChunk {
                sequence: (start_sequence + chunks.len()) as i64,
                preprocessed_text: core.to_string(),
                source_text: core.to_string(),
                map_json: map.to_json()?,
            });
        }
    }
    Ok(chunks)
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

fn restore_html_dom_chunk(
    map: &PlaceholderMap,
    after_translate_text: &str,
) -> Result<String, String> {
    let units = html_unit_descriptors(map)?;
    let mut restored = Vec::new();
    for unit in units {
        let unit_text = extract_tagged_text(after_translate_text, &unit.tag).ok_or_else(|| {
            format!(
                "Translated HTML chunk is missing expected unit tag <{}>",
                unit.tag
            )
        })?;
        for entry in text_entries_for_unit(map, &unit.tag) {
            extract_tagged_text(&unit_text, &entry.id).ok_or_else(|| {
                format!(
                    "Translated HTML unit <{}> is missing expected text node tag <{}>",
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

fn html_text_replacements(chunks: &[RenderedChunk]) -> Result<BTreeMap<usize, String>, String> {
    let mut collected = Vec::<(i64, usize, usize, String)>::new();
    let mut order = 0_usize;

    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind != HTML_DOM_CHUNK_KIND {
            continue;
        }
        let units = html_unit_descriptors(&map)?;
        for unit in units {
            let unit_text = extract_tagged_text(&chunk.after_translate_text, &unit.tag)
                .ok_or_else(|| {
                    format!(
                        "Translated HTML chunk is missing expected unit tag <{}>",
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
                    text_node_index_from_native_ref(entry.native_ref.as_deref().unwrap_or(""))
                        .unwrap_or(unit.node_index);
                let text = extract_tagged_text(&unit_text, &entry.id).ok_or_else(|| {
                    format!(
                        "Translated HTML unit <{}> is missing expected text node tag <{}>",
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

fn html_attribute_replacements(
    chunks: &[RenderedChunk],
) -> Result<Vec<HtmlAttributeReplacement>, String> {
    let mut replacements = Vec::new();
    for chunk in chunks {
        let map = super::parse_map(&chunk.map_json)?;
        if map.block_ref.kind != HTML_ATTRIBUTE_KIND {
            continue;
        }
        let Some(pointer) = map.block_ref.pointer.as_deref() else {
            return Err("HTML attribute chunk is missing pointer".into());
        };
        let Some((node_index, attr_name)) = parse_attribute_pointer(pointer) else {
            return Err(format!("Invalid HTML attribute pointer `{pointer}`"));
        };
        replacements.push(HtmlAttributeReplacement {
            node_index,
            attr_name,
            text: chunk.translated_text.clone(),
        });
    }
    Ok(replacements)
}

fn html_unit_descriptors(map: &PlaceholderMap) -> Result<Vec<HtmlUnitDescriptor>, String> {
    let units = map
        .entries
        .iter()
        .filter(|entry| entry.kind == HTML_TEXT_BLOCK_KIND)
        .map(|entry| {
            let node_index = entry
                .native_ref
                .as_deref()
                .and_then(node_index_from_native_ref)
                .ok_or_else(|| format!("HTML unit {} is missing node reference", entry.id))?;
            Ok(HtmlUnitDescriptor {
                tag: entry.id.clone(),
                node_index,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if units.is_empty() {
        return Err("HTML DOM chunk has no text block entries".into());
    }
    Ok(units)
}

fn text_entries_for_unit<'a>(map: &'a PlaceholderMap, unit_tag: &str) -> Vec<&'a PlaceholderEntry> {
    map.entries
        .iter()
        .filter(|entry| {
            entry.kind == HTML_TEXT_NODE_KIND
                && entry
                    .native_ref
                    .as_deref()
                    .is_some_and(|native_ref| native_ref.starts_with(&format!("unit:{unit_tag};")))
        })
        .collect()
}

fn apply_text_replacements(
    document: &mut Html,
    replacements: BTreeMap<usize, String>,
) -> Result<(), String> {
    if replacements.is_empty() {
        return Ok(());
    }
    let node_ids = document
        .tree
        .nodes()
        .map(|node| node.id())
        .collect::<Vec<_>>();
    for (node_index, replacement) in replacements {
        let Some(node_id) = node_ids.get(node_index).copied() else {
            return Err(format!("HTML text node index {node_index} is out of range"));
        };
        let mut node = document
            .tree
            .get_mut(node_id)
            .ok_or_else(|| format!("Unable to resolve HTML text node {node_index}"))?;
        let Node::Text(text) = node.value() else {
            return Err(format!("HTML node {node_index} is not a text node"));
        };
        let original = text.text.to_string();
        let (prefix, _, suffix) = split_core(&original).unwrap_or(("", "", ""));
        text.text = StrTendril::from_slice(&format!("{prefix}{replacement}{suffix}"));
    }
    Ok(())
}

fn apply_attribute_replacements(
    document: &mut Html,
    replacements: Vec<HtmlAttributeReplacement>,
) -> Result<(), String> {
    if replacements.is_empty() {
        return Ok(());
    }
    let node_ids = document
        .tree
        .nodes()
        .map(|node| node.id())
        .collect::<Vec<_>>();
    for replacement in replacements {
        let Some(node_id) = node_ids.get(replacement.node_index).copied() else {
            return Err(format!(
                "HTML attribute node index {} is out of range",
                replacement.node_index
            ));
        };
        let mut node = document.tree.get_mut(node_id).ok_or_else(|| {
            format!(
                "Unable to resolve HTML attribute node {}",
                replacement.node_index
            )
        })?;
        let Node::Element(element) = node.value() else {
            return Err(format!(
                "HTML node {} is not an element node",
                replacement.node_index
            ));
        };
        let Some((_, value)) = element
            .attrs
            .iter_mut()
            .find(|(name, _)| name.local.as_ref() == replacement.attr_name.as_str())
        else {
            return Err(format!(
                "HTML element node {} is missing `{}` attribute",
                replacement.node_index, replacement.attr_name
            ));
        };
        let original = value.to_string();
        let (prefix, _, suffix) = split_core(&original).unwrap_or(("", "", ""));
        *value = StrTendril::from_slice(&format!("{prefix}{}{suffix}", replacement.text));
    }
    Ok(())
}

fn node_index_from_native_ref(native_ref: &str) -> Option<usize> {
    native_ref
        .split(';')
        .find_map(|part| part.strip_prefix("node:"))
        .and_then(|index| index.parse::<usize>().ok())
}

fn text_node_index_from_native_ref(native_ref: &str) -> Option<usize> {
    node_index_from_native_ref(native_ref)
}

fn parse_attribute_pointer(pointer: &str) -> Option<(usize, String)> {
    let (node, attr) = pointer.split_once(";attr:")?;
    let node_index = node.strip_prefix("node:")?.parse::<usize>().ok()?;
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
    let pattern = Regex::new(r"</?t\d+>").expect("static HTML text placeholder strip regex");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_html_uses_dom_text_node_placeholders() {
        let text = "Before <strong>Hello</strong> after";
        let chunks = parse_html_text(text, 800).expect("parse html");
        let sources = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<Vec<_>>();
        let source = sources.join("\n");

        assert!(source.contains("<it0><t1>Before</t1></it0>"));
        assert!(source.contains(">Hello<"));
        assert!(source.contains(">after<"));
    }

    #[test]
    fn skips_non_visible_head_script_style_link_meta_and_template_text() {
        let text = concat!(
            "<html><head>",
            "<title>Visible title</title>",
            "<meta name=\"description\" content=\"Skip me\">",
            "<link title=\"Skip link\" href=\"x.css\">",
            "<style>.x{content:'Skip style'}</style>",
            "<script>const text = 'Skip script';</script>",
            "<template>Skip template</template>",
            "</head><body><p>Body text</p><template>Hidden body template</template></body></html>"
        );
        let chunks = parse_html_text(text, 800).expect("parse html");
        let sources = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<Vec<_>>();

        assert!(sources
            .iter()
            .any(|source| source.contains("Visible title")));
        assert!(sources.iter().any(|source| source.contains("Body text")));
        assert!(!sources.iter().any(|source| source.contains("Skip me")));
        assert!(!sources.iter().any(|source| source.contains("Skip link")));
        assert!(!sources.iter().any(|source| source.contains("Skip style")));
        assert!(!sources.iter().any(|source| source.contains("Skip script")));
        assert!(!sources
            .iter()
            .any(|source| source.contains("Skip template")));
        assert!(!sources
            .iter()
            .any(|source| source.contains("Hidden body template")));
    }

    #[test]
    fn ignores_angle_brackets_inside_script_and_style_raw_text() {
        let text = concat!(
            "<style>.x::before{content:'<p>Skip style</p>'}</style>",
            "<script>if (a < b) document.write('<span>Skip script</span>');</script>",
            "<p>Translate me</p>"
        );
        let chunks = parse_html_text(text, 800).expect("parse html");
        let sources = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(sources, vec!["<it0><t1>Translate me</t1></it0>"]);
    }

    #[test]
    fn extracts_and_renders_translatable_attributes_with_dom_serialization() {
        let text = r#"<input placeholder=' Search here ' title="Search title"><img alt=Cover>"#;
        let chunks = parse_html_text(text, 800).expect("parse html");
        let rendered_chunks = chunks
            .iter()
            .map(|chunk| {
                let replacement = match chunk.source_text.as_str() {
                    "Search here" => "Buscar aqui",
                    "Search title" => "Titulo de busca",
                    "Cover" => "Capa",
                    other => other,
                };
                rendered_chunk(chunk, replacement)
            })
            .collect::<Vec<_>>();

        let rendered = render_html_document(text, &rendered_chunks).expect("render html document");

        assert!(rendered.contains(r#"placeholder=" Buscar aqui ""#));
        assert!(rendered.contains(r#"title="Titulo de busca""#));
        assert!(rendered.contains(r#"alt="Capa""#));
    }

    #[test]
    fn dom_render_preserves_nested_structure_and_element_attributes() {
        let text = concat!(
            r#"<div id="app" class="layout" style="color:red" data-v-x="1">"#,
            "<p>Hello <strong>World</strong></p>",
            "</div>",
            "<script>const value = 'World';</script>"
        );
        let chunks = parse_html_text(text, 800).expect("parse html");
        let rendered_chunks = chunks
            .iter()
            .map(|chunk| {
                let replacement = chunk
                    .source_text
                    .replace("Hello", "Hola")
                    .replace("World", "Mundo");
                rendered_chunk(chunk, &replacement)
            })
            .collect::<Vec<_>>();

        let rendered = render_html_document(text, &rendered_chunks).expect("render html document");

        assert!(
            rendered.contains(r#"<div class="layout" data-v-x="1" id="app" style="color:red">"#)
        );
        assert!(rendered.contains("<p>Hola <strong>Mundo</strong></p>"));
        assert!(rendered.contains("<script>const value = 'World';</script>"));
    }

    #[test]
    fn dom_render_normalizes_broken_html_without_losing_translation() {
        let text = "<p>Hello <b>world";
        let chunks = parse_html_text(text, 800).expect("parse html");
        let rendered_chunks = chunks
            .iter()
            .map(|chunk| {
                let replacement = chunk
                    .source_text
                    .replace("Hello", "Hola")
                    .replace("world", "mundo");
                rendered_chunk(chunk, &replacement)
            })
            .collect::<Vec<_>>();

        let rendered = render_html_document(text, &rendered_chunks).expect("render html document");

        assert!(rendered.contains("<p>Hola <b>mundo</b></p>"));
    }

    fn rendered_chunk(chunk: &ParsedChunk, after_translate_text: &str) -> RenderedChunk {
        RenderedChunk {
            sequence: chunk.sequence,
            source_text: chunk.source_text.clone(),
            after_translate_text: after_translate_text.into(),
            translated_text: HtmlParser
                .restore_chunk(&chunk.map_json, after_translate_text)
                .expect("restore html chunk"),
            map_json: chunk.map_json.clone(),
        }
    }
}
