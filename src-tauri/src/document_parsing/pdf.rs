use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::markdown::parse_markdown_text_with_progress;
use super::types::ParsedChunk;
use super::DocumentParser;

pub struct PdfParser;

impl DocumentParser for PdfParser {
    fn parse(&self, input: super::types::ParserInput<'_, '_>) -> Result<Vec<ParsedChunk>, String> {
        let document = pdf_oxide::PdfDocument::open(input.source_path)
            .map_err(|error| format!("Unable to open PDF: {error}"))?;
        let page_count = document
            .page_count()
            .map_err(|error| format!("Unable to read PDF page count: {error}"))?;
        let mut markdown = String::new();
        for page_index in 0..page_count {
            if page_index > 0 {
                markdown.push_str("\n\n");
            }
            markdown.push_str(&format!("<!-- Page {} -->\n\n", page_index + 1));
            markdown.push_str(
                document
                    .extract_text(page_index)
                    .map_err(|error| format!("Unable to extract PDF text: {error}"))?
                    .trim(),
            );
        }
        let mut chunks =
            parse_markdown_text_with_progress(&markdown, input.token_limit, input.progress)?;
        for chunk in &mut chunks {
            let mut map = super::parse_map(&chunk.map_json)?;
            map.format = DocumentFormat::Pdf;
            map.content_format = ContentFormat::Markdown;
            chunk.map_json = map.to_json()?;
        }
        Ok(chunks)
    }
}
