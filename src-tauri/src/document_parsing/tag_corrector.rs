use std::collections::{HashMap, HashSet};

use regex::{Captures, Regex};

use super::types::{PlaceholderEntry, PlaceholderMap};

#[derive(Debug, Clone, PartialEq, Eq)]
enum TagToken {
    Open(String),
    Close(String),
}

pub fn correct_and_restore(translated: &str, map: &PlaceholderMap) -> String {
    let normalized = normalize_placeholder_tags(translated);
    let entries = restorable_entries(map);
    if entries.is_empty() {
        return with_block_affixes(map, &normalized);
    }
    if validate_placeholder_tags(&normalized, &entries) {
        with_block_affixes(map, &restore_native_tags(&normalized, &entries))
    } else {
        with_block_affixes(map, &strip_placeholder_tags(&normalized))
    }
}

fn normalize_placeholder_tags(text: &str) -> String {
    let pattern = Regex::new(r"(?i)<\s*(/?)\s*t\s*([0-9]+|[零〇一二三四五六七八九十两]+)\s*>")
        .expect("static placeholder normalization regex");
    pattern
        .replace_all(text, |captures: &Captures<'_>| {
            let closing = captures.get(1).is_some_and(|value| value.as_str() == "/");
            let raw_number = captures
                .get(2)
                .map(|value| value.as_str())
                .unwrap_or_default();
            let Some(index) = placeholder_index(raw_number) else {
                return captures
                    .get(0)
                    .map(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
            };
            if closing {
                format!("</t{index}>")
            } else {
                format!("<t{index}>")
            }
        })
        .to_string()
}

fn placeholder_index(value: &str) -> Option<usize> {
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value.parse::<usize>().ok();
    }
    chinese_number(value)
}

fn chinese_number(value: &str) -> Option<usize> {
    if value.is_empty() {
        return None;
    }
    if !value.contains('十') {
        return value.chars().try_fold(0_usize, |acc, character| {
            chinese_digit(character).map(|digit| acc * 10 + digit)
        });
    }

    let mut parts = value.split('十');
    let tens_part = parts.next().unwrap_or_default();
    let ones_part = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return None;
    }

    let tens = if tens_part.is_empty() {
        1
    } else {
        parse_chinese_digits(tens_part)?
    };
    let ones = if ones_part.is_empty() {
        0
    } else {
        parse_chinese_digits(ones_part)?
    };
    Some(tens * 10 + ones)
}

fn parse_chinese_digits(value: &str) -> Option<usize> {
    value.chars().try_fold(0_usize, |acc, character| {
        chinese_digit(character).map(|digit| acc * 10 + digit)
    })
}

fn chinese_digit(character: char) -> Option<usize> {
    match character {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

fn restorable_entries(map: &PlaceholderMap) -> Vec<&PlaceholderEntry> {
    map.entries
        .iter()
        .filter(|entry| !(entry.open.is_empty() && entry.close.is_empty()))
        .collect()
}

fn validate_placeholder_tags(text: &str, entries: &[&PlaceholderEntry]) -> bool {
    let expected_ids: HashSet<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for id in &expected_ids {
        counts.insert(id, (0, 0));
    }

    let mut stack = Vec::new();
    for token in placeholder_tokens(text) {
        match token {
            TagToken::Open(id) => {
                let Some(count) = counts.get_mut(id.as_str()) else {
                    return false;
                };
                count.0 += 1;
                stack.push(id);
            }
            TagToken::Close(id) => {
                let Some(count) = counts.get_mut(id.as_str()) else {
                    return false;
                };
                count.1 += 1;
                if stack.pop().as_deref() != Some(id.as_str()) {
                    return false;
                }
            }
        }
    }

    stack.is_empty()
        && entries.iter().all(|entry| {
            counts
                .get(entry.id.as_str())
                .is_some_and(|(opens, closes)| *opens == 1 && *closes == 1)
        })
}

fn placeholder_tokens(text: &str) -> Vec<TagToken> {
    let pattern = Regex::new(r"</?(t\d+)>").expect("static placeholder scan regex");
    pattern
        .captures_iter(text)
        .filter_map(|captures| {
            let full = captures.get(0)?.as_str();
            let id = captures.get(1)?.as_str().to_string();
            if full.starts_with("</") {
                Some(TagToken::Close(id))
            } else {
                Some(TagToken::Open(id))
            }
        })
        .collect()
}

fn restore_native_tags(text: &str, entries: &[&PlaceholderEntry]) -> String {
    let mut restored = text.to_string();
    for entry in entries {
        let pattern = Regex::new(&format!(
            r"(?s)<{}>(.*?)</{}>",
            regex::escape(&entry.id),
            regex::escape(&entry.id)
        ))
        .expect("escaped placeholder id should build a regex");
        restored = pattern
            .replace_all(&restored, |captures: &Captures<'_>| {
                format!("{}{}{}", entry.open, &captures[1], entry.close)
            })
            .to_string();
    }
    restored
}

fn strip_placeholder_tags(text: &str) -> String {
    let pattern = Regex::new(r"</?t\d+>").expect("static placeholder strip regex");
    pattern.replace_all(text, "").to_string()
}

fn with_block_affixes(map: &PlaceholderMap, text: &str) -> String {
    format!("{}{}{}", map.block_ref.prefix, text, map.block_ref.suffix)
}

#[cfg(test)]
mod tests {
    use crate::document_parsing::placeholders::PlaceholderBuilder;
    use crate::document_parsing::types::BlockRef;
    use crate::task_prompt::{ContentFormat, DocumentFormat};

    use super::*;

    fn map_with_two_tags() -> PlaceholderMap {
        let mut builder = PlaceholderBuilder::new(
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        );
        builder.wrap("strong", "<strong>", "</strong>");
        builder.wrap("em", "<em>", "</em>");
        builder.map()
    }

    #[test]
    fn normalizes_case_and_spaces_before_restore() {
        let mut builder = PlaceholderBuilder::new(
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        );
        builder.wrap("strong", "<strong>", "</strong>");
        let restored = correct_and_restore("点 <T 1>这里</ t1>", &builder.map());
        assert_eq!(restored, "点 <strong>这里</strong>");
    }

    #[test]
    fn normalizes_chinese_number_tags_before_restore() {
        let map = map_with_two_tags();
        let restored = correct_and_restore("<t一>红</t一> 和 <t二>蓝</t二>", &map);
        assert_eq!(restored, "<strong>红</strong> 和 <em>蓝</em>");
    }

    #[test]
    fn restores_valid_nested_tags() {
        let map = map_with_two_tags();
        let restored = correct_and_restore("<t1>红 <t2>蓝</t2></t1>", &map);
        assert_eq!(restored, "<strong>红 <em>蓝</em></strong>");
    }

    #[test]
    fn strips_tags_for_unknown_id() {
        let map = map_with_two_tags();
        let restored = correct_and_restore("<t1>红</t1> <t99>蓝</t99>", &map);
        assert_eq!(restored, "红 蓝");
    }

    #[test]
    fn strips_tags_for_missing_pair() {
        let mut builder = PlaceholderBuilder::new(
            DocumentFormat::Html,
            ContentFormat::Html,
            BlockRef::whole_document(),
        );
        builder.wrap("strong", "<strong>", "</strong>");
        let restored = correct_and_restore("<t1>没有闭合", &builder.map());
        assert_eq!(restored, "没有闭合");
    }

    #[test]
    fn strips_tags_for_crossed_nesting() {
        let map = map_with_two_tags();
        let restored = correct_and_restore("<t1><t2>坏掉</t1></t2>", &map);
        assert_eq!(restored, "坏掉");
    }

    #[test]
    fn preserves_affixes_during_fallback() {
        let mut map = map_with_two_tags();
        map.block_ref.prefix = "[00:12.34]".into();
        let restored = correct_and_restore("<t1>歌词</t1> <t99>坏</t99>", &map);
        assert_eq!(restored, "[00:12.34]歌词 坏");
    }

    #[test]
    fn leaves_text_without_entries_unchanged() {
        let map = PlaceholderMap::empty(
            DocumentFormat::Txt,
            ContentFormat::PlainText,
            BlockRef::whole_document(),
        );
        assert_eq!(correct_and_restore("纯文本", &map), "纯文本");
    }
}
