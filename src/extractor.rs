use anyhow::{Result, anyhow};
use quick_xml::events::Event;
use quick_xml::reader::Reader as XmlReader;
use std::io::{Cursor, Read};

#[derive(Debug, Clone)]
pub struct ExtractedChunkSource {
    pub page_no: Option<i32>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ExtractedDocument {
    pub mime_type: String,
    pub sections: Vec<ExtractedChunkSource>,
}

pub fn extract_text(file_name: &str, data: &[u8]) -> Result<ExtractedDocument> {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt") {
        extract_plain_text(data, "text/markdown")
    } else if lower.ends_with(".json") {
        extract_json(data)
    } else if lower.ends_with(".csv") {
        extract_csv(data)
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        extract_html(data)
    } else if lower.ends_with(".pdf") {
        extract_pdf(data)
    } else if lower.ends_with(".docx") {
        extract_docx(data)
    } else {
        // Fallback: try UTF-8 text
        extract_plain_text(data, "application/octet-stream")
    }
}

fn extract_plain_text(data: &[u8], mime: &str) -> Result<ExtractedDocument> {
    let text = String::from_utf8_lossy(data).to_string();
    Ok(ExtractedDocument {
        mime_type: mime.to_string(),
        sections: vec![ExtractedChunkSource {
            page_no: None,
            text,
        }],
    })
}

fn extract_json(data: &[u8]) -> Result<ExtractedDocument> {
    let val: serde_json::Value = serde_json::from_slice(data)?;
    let text = serde_json::to_string_pretty(&val)?;
    Ok(ExtractedDocument {
        mime_type: "application/json".to_string(),
        sections: vec![ExtractedChunkSource {
            page_no: None,
            text,
        }],
    })
}

fn extract_csv(data: &[u8]) -> Result<ExtractedDocument> {
    let mut reader = csv::Reader::from_reader(data);
    let mut lines = Vec::new();
    let headers = reader.headers()?.clone();
    lines.push(headers.iter().collect::<Vec<_>>().join(" | "));

    for result in reader.records() {
        let record = result?;
        lines.push(record.iter().collect::<Vec<_>>().join(" | "));
    }

    Ok(ExtractedDocument {
        mime_type: "text/csv".to_string(),
        sections: vec![ExtractedChunkSource {
            page_no: None,
            text: lines.join("\n"),
        }],
    })
}

fn extract_html(data: &[u8]) -> Result<ExtractedDocument> {
    let raw = String::from_utf8_lossy(data);
    let mut reader = XmlReader::from_str(&raw);
    reader.config_mut().check_end_names = false;
    let mut text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                if let Ok(unescaped) = e.unescape() {
                    let trimmed = unescaped.trim();
                    if !trimmed.is_empty() {
                        text.push_str(trimmed);
                        text.push(' ');
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break, // Best-effort parsing on malformed HTML
            _ => (),
        }
        buf.clear();
    }

    if text.is_empty() {
        text = raw.to_string();
    }

    Ok(ExtractedDocument {
        mime_type: "text/html".to_string(),
        sections: vec![ExtractedChunkSource {
            page_no: None,
            text,
        }],
    })
}

fn extract_pdf(data: &[u8]) -> Result<ExtractedDocument> {
    let doc = lopdf::Document::load_mem(data)
        .map_err(|e| anyhow!("failed to parse PDF document: {}", e))?;
    let mut sections = Vec::new();

    for (page_num, _page_id) in doc.get_pages() {
        if let Ok(text) = doc.extract_text(&[page_num]) {
            let clean = text.trim();
            if !clean.is_empty() {
                sections.push(ExtractedChunkSource {
                    page_no: Some(page_num as i32),
                    text: clean.to_string(),
                });
            }
        }
    }

    if sections.is_empty() {
        sections.push(ExtractedChunkSource {
            page_no: Some(1),
            text: String::from_utf8_lossy(data).to_string(),
        });
    }

    Ok(ExtractedDocument {
        mime_type: "application/pdf".to_string(),
        sections,
    })
}

fn extract_docx(data: &[u8]) -> Result<ExtractedDocument> {
    let reader = Cursor::new(data);
    let mut zip = zip::ZipArchive::new(reader)
        .map_err(|e| anyhow!("failed to open docx zip archive: {}", e))?;

    let mut xml_content = String::new();
    {
        let mut file = zip
            .by_name("word/document.xml")
            .map_err(|e| anyhow!("docx missing word/document.xml: {}", e))?;
        file.read_to_string(&mut xml_content)?;
    }

    let mut xml_reader = XmlReader::from_str(&xml_content);
    let mut buf = Vec::new();
    let mut paragraphs = Vec::new();
    let mut current_p = String::new();

    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                if let Ok(txt) = e.unescape() {
                    current_p.push_str(&txt);
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"w:p" => {
                let trimmed = current_p.trim();
                if !trimmed.is_empty() {
                    paragraphs.push(trimmed.to_string());
                }
                current_p.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => (),
        }
        buf.clear();
    }

    Ok(ExtractedDocument {
        mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            .to_string(),
        sections: vec![ExtractedChunkSource {
            page_no: None,
            text: paragraphs.join("\n\n"),
        }],
    })
}
