use crate::extractor::ExtractedDocument;

#[derive(Debug, Clone)]
pub struct DocumentChunk {
    pub chunk_index: i32,
    pub page_no: Option<i32>,
    pub chunk_text: String,
    pub token_count: i32,
    pub start_char_offset: i32,
    pub end_char_offset: i32,
}

pub fn chunk_document(
    doc: &ExtractedDocument,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Vec<DocumentChunk> {
    let mut chunks = Vec::new();
    let mut current_index = 0;

    for section in &doc.sections {
        let text = section.text.trim();
        if text.is_empty() {
            continue;
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }

        let step = if chunk_size > chunk_overlap {
            chunk_size - chunk_overlap
        } else {
            chunk_size
        };

        let mut start_idx = 0;
        while start_idx < words.len() {
            let end_idx = (start_idx + chunk_size).min(words.len());
            let chunk_words = &words[start_idx..end_idx];
            let chunk_str = chunk_words.join(" ");

            // Estimate character offsets
            let start_char = 0; // relative chunk
            let end_char = chunk_str.len();

            let estimated_tokens = (chunk_words.len() as f32 * 1.3) as i32;

            chunks.push(DocumentChunk {
                chunk_index: current_index,
                page_no: section.page_no,
                chunk_text: chunk_str,
                token_count: estimated_tokens.max(1),
                start_char_offset: start_char as i32,
                end_char_offset: end_char as i32,
            });

            current_index += 1;
            if end_idx == words.len() {
                break;
            }
            start_idx += step;
        }
    }

    chunks
}
