use std::fs::File;
use std::io::{Cursor, Read, Write};

use quick_xml::events::Event;
use quick_xml::{Reader, Writer};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::types::{BlockRef, ParsedChunk, PlaceholderMap, RenderInput};
use super::DocumentParser;

const DOCUMENT_XML: &str = "word/document.xml";

pub struct DocxParser;

impl DocumentParser for DocxParser {
    fn parse(&self, input: super::types::ParserInput<'_>) -> Result<Vec<ParsedChunk>, String> {
        let xml = read_zip_text(input.source_path, DOCUMENT_XML)?;
        let paragraphs = extract_word_paragraphs(&xml)?;
        paragraphs
            .into_iter()
            .enumerate()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(index, text)| {
                let map = PlaceholderMap::empty(
                    DocumentFormat::Docx,
                    ContentFormat::Xml,
                    BlockRef {
                        kind: "docx-paragraph".into(),
                        path: Some(DOCUMENT_XML.into()),
                        index: Some(index),
                        pointer: None,
                        prefix: String::new(),
                        suffix: String::new(),
                    },
                );
                Ok(ParsedChunk {
                    sequence: index as i64,
                    preprocessed_text: text.clone(),
                    source_text: text,
                    map_json: map.to_json()?,
                })
            })
            .collect()
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
                let replacements = input
                    .chunks
                    .iter()
                    .filter_map(|chunk| {
                        let map = super::parse_map(&chunk.map_json).ok()?;
                        (map.block_ref.path.as_deref() == Some(DOCUMENT_XML))
                            .then_some((map.block_ref.index?, chunk.translated_text.clone()))
                    })
                    .collect::<Vec<_>>();
                let rendered = replace_word_paragraphs(&xml, &replacements)?;
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

fn read_zip_text(path: &std::path::Path, entry: &str) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("Unable to open ZIP source: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut file = archive.by_name(entry).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

fn extract_word_paragraphs(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut paragraphs = Vec::new();
    let mut in_paragraph = false;
    let mut current = String::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?
        {
            Event::Start(event)
                if event.name().as_ref().ends_with(b":p") || event.name().as_ref() == b"w:p" =>
            {
                in_paragraph = true;
                current.clear();
            }
            Event::End(event)
                if event.name().as_ref().ends_with(b":p") || event.name().as_ref() == b"w:p" =>
            {
                if in_paragraph {
                    paragraphs.push(current.clone());
                }
                in_paragraph = false;
            }
            Event::Text(text) if in_paragraph => {
                current.push_str(&text.unescape().map_err(|error| error.to_string())?);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(paragraphs)
}

fn replace_word_paragraphs(xml: &str, replacements: &[(usize, String)]) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut paragraph_index = 0_usize;
    let mut in_target = false;
    let mut wrote_target_text = false;
    let mut replacement = None::<String>;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| error.to_string())?;
        match event {
            Event::Start(ref event)
                if event.name().as_ref().ends_with(b":p") || event.name().as_ref() == b"w:p" =>
            {
                replacement = replacements
                    .iter()
                    .find_map(|(index, text)| (*index == paragraph_index).then(|| text.clone()));
                in_target = replacement.is_some();
                wrote_target_text = false;
                writer
                    .write_event(Event::Start(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::End(ref event)
                if event.name().as_ref().ends_with(b":p") || event.name().as_ref() == b"w:p" =>
            {
                paragraph_index += 1;
                in_target = false;
                writer
                    .write_event(Event::End(event.clone()))
                    .map_err(|error| error.to_string())?;
            }
            Event::Text(_) if in_target => {
                if !wrote_target_text {
                    writer
                        .write_event(Event::Text(quick_xml::events::BytesText::new(
                            replacement.as_deref().unwrap_or_default(),
                        )))
                        .map_err(|error| error.to_string())?;
                    wrote_target_text = true;
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
