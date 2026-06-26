use std::fs::File;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::html::{parse_html_text, render_html_document};
use super::types::{ParsedChunk, RenderInput, RenderedChunk};
use super::DocumentParser;

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
            let mut parsed = parse_html_text(
                &text,
                DocumentFormat::Epub,
                ContentFormat::Xhtml,
                Some(name),
                input.token_limit,
            )?;
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
    render_html_document(&original_text, chunks, Some(name))
}

fn resequence(chunks: &mut [ParsedChunk]) {
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
    }
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
        let parsed = parse_html_text(
            "<p>Hello <strong>World</strong></p>",
            DocumentFormat::Epub,
            ContentFormat::Xhtml,
            Some("OEBPS/chapter.xhtml".into()),
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
