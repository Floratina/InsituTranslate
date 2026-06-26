use crate::task_prompt::{ContentFormat, DocumentFormat};
use comrak::nodes::{AstNode, NodeCode, NodeValue, Sourcepos};
use comrak::{parse_document, Arena, Options};

use super::tag_corrector::correct_and_restore;
use super::types::{BlockRef, PlaceholderEntry, PlaceholderMap};

pub const BLOCKQUOTE_PREFIX_KIND: &str = "blockquote-prefix";
pub const MARKDOWN_CODE_BLOCK_KIND: &str = "markdown-code-block";

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

    pub fn record_non_translatable(
        &mut self,
        kind: &str,
        original: impl Into<String>,
        native_ref: Option<String>,
    ) -> String {
        let id = self.next_id();
        self.map.entries.push(PlaceholderEntry {
            id: id.clone(),
            kind: kind.into(),
            original: original.into(),
            open: String::new(),
            close: String::new(),
            translatable: false,
            native_ref,
        });
        id
    }

    pub fn map(self) -> PlaceholderMap {
        self.map
    }

    fn next_id(&mut self) -> String {
        let id = format!("t{}", self.next_index);
        self.next_index += 1;
        id
    }
}

#[derive(Debug)]
pub struct PlaceholderManager {
    builder: PlaceholderBuilder,
}

impl PlaceholderManager {
    pub fn new(format: DocumentFormat, content_format: ContentFormat, block_ref: BlockRef) -> Self {
        Self {
            builder: PlaceholderBuilder::new(format, content_format, block_ref),
        }
    }

    pub fn protect_markdown(mut self, text: &str) -> Result<(String, String), String> {
        let without_blockquote_prefixes = self.protect_markdown_blockquote_prefixes(text);
        let without_code_blocks = self.protect_markdown_code_blocks(&without_blockquote_prefixes);
        let source = self.protect_markdown_inline(&without_code_blocks)?;
        Ok((source, self.builder.map().to_json()?))
    }

    pub fn protect_html(mut self, text: &str) -> Result<(String, String), String> {
        let source = self.protect_html_inline(text)?;
        Ok((source, self.builder.map().to_json()?))
    }

    fn protect_markdown_blockquote_prefixes(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        for (line_index, line) in text.split_inclusive('\n').enumerate() {
            if let Some(clean_line) = line.strip_prefix("> ") {
                self.builder.record_non_translatable(
                    BLOCKQUOTE_PREFIX_KIND,
                    "> ",
                    Some(format!("line:{line_index}")),
                );
                output.push_str(clean_line);
            } else {
                output.push_str(line);
            }
        }
        output
    }

    fn protect_markdown_inline(&mut self, text: &str) -> Result<String, String> {
        let replacements = markdown_inline_replacements(text);
        let mut output = String::with_capacity(text.len());
        let mut last_end = 0_usize;
        for replacement in replacements {
            output.push_str(&text[last_end..replacement.start]);
            let id = self
                .builder
                .wrap(&replacement.kind, replacement.open, replacement.close);
            output.push_str(&format!("<{id}>{}</{id}>", replacement.inner));
            last_end = replacement.end;
        }
        output.push_str(&text[last_end..]);
        Ok(output)
    }

    fn protect_markdown_code_blocks(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        let mut code_block = String::new();
        let mut fence = None::<String>;

        for line in text.split_inclusive('\n') {
            if let Some(active_fence) = fence.as_deref() {
                code_block.push_str(line);
                if markdown_fence_marker(line).as_deref() == Some(active_fence) {
                    self.push_markdown_code_block_placeholder(&mut output, &mut code_block);
                    fence = None;
                }
                continue;
            }

            if let Some(marker) = markdown_fence_marker(line) {
                fence = Some(marker);
                code_block.push_str(line);
            } else {
                output.push_str(line);
            }
        }

        if !code_block.is_empty() {
            self.push_markdown_code_block_placeholder(&mut output, &mut code_block);
        }
        output
    }

    fn push_markdown_code_block_placeholder(
        &mut self,
        output: &mut String,
        code_block: &mut String,
    ) {
        let id = self.builder.record_non_translatable(
            MARKDOWN_CODE_BLOCK_KIND,
            code_block.clone(),
            None,
        );
        output.push_str(&format!("<{id}></{id}>"));
        code_block.clear();
    }

    fn protect_html_inline(&mut self, text: &str) -> Result<String, String> {
        let replacements = html_inline_replacements(text);
        let mut output = String::with_capacity(text.len());
        let mut last_end = 0_usize;
        for replacement in replacements {
            output.push_str(&text[last_end..replacement.start]);
            let id = self
                .builder
                .wrap("markup", replacement.open, replacement.close);
            output.push_str(&format!("<{id}>{}</{id}>", replacement.inner));
            last_end = replacement.end;
        }
        output.push_str(&text[last_end..]);
        Ok(output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownInlineReplacement {
    start: usize,
    end: usize,
    kind: String,
    open: String,
    close: String,
    inner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlInlineReplacement {
    start: usize,
    end: usize,
    open: String,
    close: String,
    inner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlInlineTag {
    name: String,
    start: usize,
    end: usize,
    closing: bool,
    self_closing: bool,
}

fn html_inline_replacements(text: &str) -> Vec<HtmlInlineReplacement> {
    let mut replacements = Vec::new();
    let mut stack = Vec::<HtmlInlineTag>::new();
    let mut consumed_until = 0_usize;
    let mut index = 0_usize;

    while index < text.len() {
        let Some(tag_offset) = text[index..].find('<') else {
            break;
        };
        let tag_start = index + tag_offset;
        let Some(tag) = parse_html_inline_tag(text, tag_start) else {
            break;
        };
        index = tag.end;

        if tag.name.is_empty() || tag.self_closing {
            continue;
        }
        if tag.closing {
            let Some(open_index) = stack.iter().rposition(|open| open.name == tag.name) else {
                stack.clear();
                continue;
            };
            let open = stack.remove(open_index);
            stack.truncate(open_index);
            if open.start < consumed_until || open.end > tag.start {
                continue;
            }
            let Some(open_text) = text.get(open.start..open.end) else {
                continue;
            };
            let Some(close_text) = text.get(tag.start..tag.end) else {
                continue;
            };
            let Some(inner) = text.get(open.end..tag.start) else {
                continue;
            };
            if inner.trim().is_empty() {
                continue;
            }
            replacements.push(HtmlInlineReplacement {
                start: open.start,
                end: tag.end,
                open: open_text.to_string(),
                close: close_text.to_string(),
                inner: inner.to_string(),
            });
            consumed_until = tag.end;
            stack.clear();
        } else if is_html_placeholder_tag(&tag.name) {
            stack.push(tag);
        }
    }

    replacements
}

fn parse_html_inline_tag(text: &str, start: usize) -> Option<HtmlInlineTag> {
    if text.get(start..)?.starts_with("<!--")
        || text.get(start..)?.starts_with("<!")
        || text.get(start..)?.starts_with("<?")
    {
        return None;
    }
    let end = find_html_tag_end(text, start)?;
    let raw = text.get(start..end)?;
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
        return None;
    }
    let name = raw[name_start..cursor].to_ascii_lowercase();
    let self_closing = raw[..raw.len().saturating_sub(1)].trim_end().ends_with('/');
    Some(HtmlInlineTag {
        name,
        start,
        end,
        closing,
        self_closing,
    })
}

fn find_html_tag_end(text: &str, start: usize) -> Option<usize> {
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

fn skip_ascii_whitespace(text: &str, cursor: &mut usize) {
    while *cursor < text.len() && text.as_bytes()[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

fn is_html_placeholder_tag(name: &str) -> bool {
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

#[derive(Debug)]
struct MarkdownLineIndex {
    starts: Vec<usize>,
}

impl MarkdownLineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0_usize];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' && index + 1 <= text.len() {
                starts.push(index + 1);
            }
        }
        Self { starts }
    }

    fn line_col_to_byte_offset(&self, text: &str, line: usize, column: usize) -> Option<usize> {
        if column == 0 {
            return None;
        }
        let (line_start, line_end) = self.line_bounds(text, line)?;
        let offset = line_start.checked_add(column.checked_sub(1)?)?;
        if offset <= line_end && text.is_char_boundary(offset) {
            Some(offset)
        } else {
            None
        }
    }

    fn line_col_to_byte_offset_after(
        &self,
        text: &str,
        line: usize,
        column: usize,
    ) -> Option<usize> {
        if column == 0 {
            return None;
        }
        let (line_start, line_end) = self.line_bounds(text, line)?;
        let offset = line_start.checked_add(column)?;
        if offset <= line_end && text.is_char_boundary(offset) {
            Some(offset)
        } else {
            None
        }
    }

    fn line_bounds(&self, text: &str, line: usize) -> Option<(usize, usize)> {
        if line == 0 || line > self.starts.len() {
            return None;
        }
        let line_start = self.starts[line - 1];
        let mut line_end = self.starts.get(line).copied().unwrap_or(text.len());
        let bytes = text.as_bytes();
        if line_end > line_start && bytes.get(line_end - 1) == Some(&b'\n') {
            line_end -= 1;
            if line_end > line_start && bytes.get(line_end - 1) == Some(&b'\r') {
                line_end -= 1;
            }
        }
        Some((line_start, line_end))
    }
}

fn markdown_inline_replacements(text: &str) -> Vec<MarkdownInlineReplacement> {
    if text.is_empty() {
        return Vec::new();
    }

    let arena = Arena::new();
    let options = markdown_options();
    let root = parse_document(&arena, text, &options);
    let line_index = MarkdownLineIndex::new(text);
    let mut replacements = root
        .descendants()
        .filter_map(|node| markdown_replacement_for_node(text, &line_index, node))
        .collect::<Vec<_>>();
    replacements.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut filtered = Vec::new();
    let mut last_end = 0_usize;
    for replacement in replacements {
        if replacement.start < last_end {
            continue;
        }
        last_end = replacement.end;
        filtered.push(replacement);
    }
    filtered
}

fn markdown_fence_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let marker = if trimmed.starts_with("```") {
        '`'
    } else if trimmed.starts_with("~~~") {
        '~'
    } else {
        return None;
    };
    let count = trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count();
    (count >= 3).then(|| marker.to_string().repeat(count))
}

fn markdown_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.front_matter_delimiter = Some("---".to_string());
    options.render.sourcepos = true;
    options.render.experimental_inline_sourcepos = true;
    options
}

fn markdown_replacement_for_node(
    text: &str,
    line_index: &MarkdownLineIndex,
    node: &AstNode<'_>,
) -> Option<MarkdownInlineReplacement> {
    let data = node.data.borrow();
    if let NodeValue::Code(code) = &data.value {
        return markdown_code_replacement(text, line_index, data.sourcepos, code);
    }

    let (start, end) = sourcepos_to_byte_range(text, line_index, data.sourcepos)?;
    let raw = text.get(start..end)?;
    let (kind, open, close, inner) = match &data.value {
        NodeValue::Link(_) => markdown_link_parts(raw)?,
        NodeValue::Strong => markdown_delimited_parts(raw, "strong", &["**", "__"])?,
        NodeValue::Emph => markdown_delimited_parts(raw, "emphasis", &["*", "_"])?,
        NodeValue::Strikethrough => markdown_delimited_parts(raw, "strikethrough", &["~~"])?,
        _ => return None,
    };

    Some(MarkdownInlineReplacement {
        start,
        end,
        kind,
        open,
        close,
        inner,
    })
}

fn markdown_code_replacement(
    text: &str,
    line_index: &MarkdownLineIndex,
    sourcepos: Sourcepos,
    code: &NodeCode,
) -> Option<MarkdownInlineReplacement> {
    let (inner_start, inner_end) = sourcepos_to_byte_range(text, line_index, sourcepos)?;
    if code.num_backticks == 0
        || inner_start < code.num_backticks
        || inner_end.checked_add(code.num_backticks)? > text.len()
    {
        return None;
    }

    let start = inner_start - code.num_backticks;
    let end = inner_end + code.num_backticks;
    let open = text.get(start..inner_start)?;
    let close = text.get(inner_end..end)?;
    if !open.bytes().all(|byte| byte == b'`') || !close.bytes().all(|byte| byte == b'`') {
        return None;
    }

    Some(MarkdownInlineReplacement {
        start,
        end,
        kind: "code".to_string(),
        open: open.to_string(),
        close: close.to_string(),
        inner: text.get(inner_start..inner_end)?.to_string(),
    })
}

fn sourcepos_to_byte_range(
    text: &str,
    line_index: &MarkdownLineIndex,
    sourcepos: Sourcepos,
) -> Option<(usize, usize)> {
    let start =
        line_index.line_col_to_byte_offset(text, sourcepos.start.line, sourcepos.start.column)?;
    let end =
        line_index.line_col_to_byte_offset_after(text, sourcepos.end.line, sourcepos.end.column)?;
    if start < end && end <= text.len() {
        Some((start, end))
    } else {
        None
    }
}

fn markdown_link_parts(raw: &str) -> Option<(String, String, String, String)> {
    if !raw.starts_with('[') {
        return None;
    }
    let label_end = markdown_link_label_end(raw)?;
    Some((
        "link".to_string(),
        "[".to_string(),
        raw[label_end..].to_string(),
        raw[1..label_end].to_string(),
    ))
}

fn markdown_link_label_end(raw: &str) -> Option<usize> {
    let mut escaped = false;
    let mut depth = 0_usize;
    for (index, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match character {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn markdown_delimited_parts(
    raw: &str,
    kind: &str,
    delimiters: &[&str],
) -> Option<(String, String, String, String)> {
    let delimiter = delimiters
        .iter()
        .find(|delimiter| raw.starts_with(**delimiter) && raw.ends_with(**delimiter))?;
    if raw.len() < delimiter.len() * 2 {
        return None;
    }
    Some((
        kind.to_string(),
        (*delimiter).to_string(),
        (*delimiter).to_string(),
        raw[delimiter.len()..raw.len() - delimiter.len()].to_string(),
    ))
}

pub fn protect_markdown(text: &str, block_ref: BlockRef) -> Result<(String, String), String> {
    PlaceholderManager::new(DocumentFormat::Markdown, ContentFormat::Markdown, block_ref)
        .protect_markdown(text)
}

pub fn protect_html(
    text: &str,
    format: DocumentFormat,
    content_format: ContentFormat,
    block_ref: BlockRef,
) -> Result<(String, String), String> {
    PlaceholderManager::new(format, content_format, block_ref).protect_html(text)
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

#[cfg(test)]
mod manager_tests {
    use crate::document_parsing::types::BlockRef;
    use crate::task_prompt::{ContentFormat, DocumentFormat};

    use super::*;

    #[test]
    fn protects_html_tags_with_single_pass_without_reentering_generated_placeholders() {
        let (source, map_json) = protect_html(
            "Before <strong>Hello</strong> after",
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        )
        .expect("protect html");

        assert_eq!(source, "Before <t1>Hello</t1> after");
        assert!(!source.contains("<t2>"));
        assert_eq!(
            restore_from_json(&map_json, "Before <t1>你好</t1> after").expect("restore html"),
            "Before <strong>你好</strong> after"
        );
    }

    #[test]
    fn protects_sibling_html_tags_in_source_order() {
        let (source, map_json) = protect_html(
            "<strong>One</strong> and <em>Two</em>",
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        )
        .expect("protect html");

        assert_eq!(source, "<t1>One</t1> and <t2>Two</t2>");
        assert_eq!(
            restore_from_json(&map_json, "<t1>一</t1> and <t2>二</t2>").expect("restore html"),
            "<strong>一</strong> and <em>二</em>"
        );
    }

    #[test]
    fn protects_markdown_inline_rules_from_one_manager() {
        let (source, map_json) = protect_markdown(
            "**bold** `code` [docs](https://example.com)",
            BlockRef::whole_document(),
        )
        .expect("protect markdown");

        assert_eq!(source, "<t1>bold</t1> <t2>code</t2> <t3>docs</t3>");
        assert_eq!(
            restore_from_json(&map_json, "<t1>粗体</t1> <t2>code</t2> <t3>文档</t3>")
                .expect("restore markdown"),
            "**粗体** `code` [文档](https://example.com)"
        );
    }

    #[test]
    fn converts_utf8_line_columns_only_at_valid_char_boundaries() {
        let text = "a你🙂b\nnext";

        let line_index = MarkdownLineIndex::new(text);

        assert_eq!(line_index.line_col_to_byte_offset(text, 1, 1), Some(0));
        assert_eq!(line_index.line_col_to_byte_offset(text, 1, 2), Some(1));
        assert_eq!(line_index.line_col_to_byte_offset(text, 1, 5), Some(4));
        assert_eq!(line_index.line_col_to_byte_offset(text, 1, 9), Some(8));
        assert_eq!(line_index.line_col_to_byte_offset(text, 1, 3), None);
        assert_eq!(line_index.line_col_to_byte_offset(text, 2, 1), Some(10));
        assert_eq!(line_index.line_col_to_byte_offset(text, 0, 1), None);
    }

    #[test]
    fn protects_markdown_inline_rules_with_utf8_text() {
        let (source, map_json) = protect_markdown(
            "**你好🙂** [文档](https://example.com) ``代码🙂``",
            BlockRef::whole_document(),
        )
        .expect("protect markdown");

        assert_eq!(source, "<t1>你好🙂</t1> <t2>文档</t2> <t3>代码🙂</t3>");
        assert_eq!(
            restore_from_json(&map_json, "<t1>您好🙂</t1> <t2>资料</t2> <t3>代码🙂</t3>")
                .expect("restore markdown"),
            "**您好🙂** [资料](https://example.com) ``代码🙂``"
        );
    }

    #[test]
    fn protects_outer_markdown_inline_node_for_nested_emphasis() {
        let (source, map_json) = protect_markdown("**bold and *em***", BlockRef::whole_document())
            .expect("protect markdown");

        assert_eq!(source, "<t1>bold and *em*</t1>");
        assert_eq!(
            restore_from_json(&map_json, "<t1>粗体和 *斜体*</t1>").expect("restore markdown"),
            "**粗体和 *斜体***"
        );
    }

    #[test]
    fn preserves_complex_link_suffix_from_original_markdown() {
        let text = r#"[**docs**](https://example.com/a_(b) "Title")"#;
        let (source, map_json) =
            protect_markdown(text, BlockRef::whole_document()).expect("protect markdown");

        assert_eq!(source, "<t1>**docs**</t1>");
        assert_eq!(
            restore_from_json(&map_json, "<t1>**文档**</t1>").expect("restore markdown"),
            r#"[**文档**](https://example.com/a_(b) "Title")"#
        );
    }

    #[test]
    fn preserves_multiple_backtick_code_span_delimiters() {
        let text = "Use ``code ` span`` here";
        let (source, map_json) =
            protect_markdown(text, BlockRef::whole_document()).expect("protect markdown");

        assert_eq!(source, "Use <t1>code ` span</t1> here");
        assert_eq!(
            restore_from_json(&map_json, "Use <t1>代码 ` span</t1> here")
                .expect("restore markdown"),
            "Use ``代码 ` span`` here"
        );
    }

    #[test]
    fn keeps_frontmatter_callouts_tables_and_code_blocks_without_rerendering() {
        let text = concat!(
            "---\n",
            "title: **Keep**\n",
            "---\n\n",
            "> [!NOTE]\n",
            "> callout\n\n",
            "| A | B |\n",
            "|---|:--|\n",
            "| one | two |\n\n",
            "```rust\n",
            "let bold = \"**no**\";\n",
            "```\n",
        );
        let (source, map_json) =
            protect_markdown(text, BlockRef::whole_document()).expect("protect markdown");

        assert!(source.contains("title: **Keep**"));
        assert!(source.contains("[!NOTE]\ncallout"));
        assert!(source.contains("|---|:--|"));
        assert!(!source.contains("let bold = \"**no**\";"));
        assert_eq!(
            restore_from_json(&map_json, &source).expect("restore markdown"),
            text
        );
    }

    #[test]
    fn protects_markdown_fenced_code_block_as_non_translatable() {
        let text = "Before\n```rust\nlet bold = \"**no**\";\n```\nAfter";
        let (source, map_json) =
            protect_markdown(text, BlockRef::whole_document()).expect("protect markdown");

        assert!(source.contains("Before"));
        assert!(source.contains("<t1></t1>"));
        assert!(!source.contains("let bold"));
        assert_eq!(
            restore_from_json(&map_json, "Avant\n<t1></t1>Apres").expect("restore markdown"),
            "Avant\n```rust\nlet bold = \"**no**\";\n```\nApres"
        );
    }

    #[test]
    fn strips_and_restores_markdown_blockquote_prefixes_by_line() {
        let (source, map_json) =
            protect_markdown("> quoted\nplain\n> second\n", BlockRef::whole_document())
                .expect("protect markdown");

        assert_eq!(source, "quoted\nplain\nsecond\n");
        assert_eq!(
            restore_from_json(&map_json, "译文\nplain\n第二\n").expect("restore blockquote"),
            "> 译文\nplain\n> 第二\n"
        );
    }
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
