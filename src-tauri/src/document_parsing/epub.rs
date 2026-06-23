use std::fs::File;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::html::parse_html_text;
use super::types::{ParsedChunk, RenderInput};
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
                let replacement = input
                    .chunks
                    .iter()
                    .find_map(|chunk| {
                        super::parse_map(&chunk.map_json).ok().and_then(|map| {
                            (map.block_ref.path.as_deref() == Some(&name))
                                .then(|| chunk.translated_text.clone())
                        })
                    })
                    .unwrap_or(text);
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

fn resequence(chunks: &mut [ParsedChunk]) {
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.sequence = index as i64;
    }
}
