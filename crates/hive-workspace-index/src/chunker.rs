/// A chunk of text with its position metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    /// Zero-based chunk index.
    pub index: usize,
    /// The chunk text.
    pub text: String,
    /// Byte offset in the original text where this chunk starts.
    pub start_byte: usize,
    /// Byte offset in the original text where this chunk ends (exclusive).
    pub end_byte: usize,
}

/// Split `text` into chunks of approximately `chunk_size` characters with
/// `overlap_pct` fractional overlap between adjacent chunks.
///
/// Splitting prefers natural boundaries: paragraph breaks (`\n\n`), then
/// newlines (`\n`), then sentence ends (`. `), then spaces.
pub fn chunk_text(text: &str, chunk_size: usize, overlap_pct: f64) -> Vec<TextChunk> {
    if text.is_empty() {
        return Vec::new();
    }
    if text.len() <= chunk_size {
        return vec![TextChunk {
            index: 0,
            text: text.to_string(),
            start_byte: 0,
            end_byte: text.len(),
        }];
    }

    let overlap = (chunk_size as f64 * overlap_pct).round() as usize;
    let step = chunk_size.saturating_sub(overlap).max(1);

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let raw_end = (start + chunk_size).min(text.len());

        // Snap to a valid UTF-8 char boundary (walk backward if needed)
        let raw_end = snap_to_char_boundary(text, raw_end);

        // If we're at the end, take the rest
        let end =
            if raw_end >= text.len() { text.len() } else { find_split_point(text, start, raw_end) };

        // Avoid empty or duplicate trailing chunks
        if end <= start {
            break;
        }

        chunks.push(TextChunk {
            index: chunks.len(),
            text: text[start..end].to_string(),
            start_byte: start,
            end_byte: end,
        });

        if end >= text.len() {
            break;
        }

        // Advance by step, but at least to a point that makes progress
        let next_start = start + step;
        if next_start <= start {
            break;
        }
        start = next_start.min(text.len());
        // Snap to a valid UTF-8 char boundary
        start = snap_to_char_boundary(text, start);
    }

    chunks
}

/// Snap a byte index to the nearest valid UTF-8 char boundary at or before `pos`.
fn snap_to_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    // Walk backward until we hit a char boundary
    let mut p = pos;
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Find the best split point near `target` within `text[start..target]`.
///
/// Prefers, in order: paragraph boundary, newline, sentence end, space.
/// Falls back to `target` if no boundary found.
fn find_split_point(text: &str, start: usize, target: usize) -> usize {
    let search_region = &text[start..target];

    // Search in the last 20% of the region for a good boundary.
    // Snap to a char boundary so we don't slice inside a multi-byte character.
    let raw_search_start = search_region.len().saturating_sub(search_region.len() / 5);
    let search_start = snap_to_char_boundary(search_region, raw_search_start);
    let tail = &search_region[search_start..];

    // Paragraph break
    if let Some(pos) = tail.rfind("\n\n") {
        return start + search_start + pos + 2; // after the break
    }
    // Newline
    if let Some(pos) = tail.rfind('\n') {
        return start + search_start + pos + 1;
    }
    // Sentence end
    if let Some(pos) = tail.rfind(". ") {
        return start + search_start + pos + 2;
    }
    // Space
    if let Some(pos) = tail.rfind(' ') {
        return start + search_start + pos + 1;
    }

    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_text_returns_single_chunk() {
        let text = "Hello, world!";
        let chunks = chunk_text(text, 2000, 0.10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, text);
        assert_eq!(chunks[0].start_byte, 0);
        assert_eq!(chunks[0].end_byte, text.len());
    }

    #[test]
    fn empty_text_returns_no_chunks() {
        let chunks = chunk_text("", 2000, 0.10);
        assert!(chunks.is_empty());
    }

    #[test]
    fn large_text_produces_multiple_chunks() {
        // Create text that is exactly 5000 chars
        let text = "a".repeat(5000);
        let chunks = chunk_text(&text, 2000, 0.10);
        assert!(chunks.len() >= 3);
    }

    #[test]
    fn chunks_have_correct_overlap() {
        // Create text with clear word boundaries
        let words: Vec<String> = (0..500).map(|i| format!("word{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text, 2000, 0.10);

        // Verify overlap: end of chunk[i] should overlap with start of chunk[i+1]
        for i in 0..chunks.len() - 1 {
            let overlap_start = chunks[i + 1].start_byte;
            let overlap_end = chunks[i].end_byte;
            // The next chunk should start before or at the end of the current chunk
            // (accounting for boundary adjustments)
            assert!(
                overlap_start < overlap_end || overlap_end - overlap_start < 400,
                "chunk {} (end={}) and chunk {} (start={}) should overlap",
                i,
                overlap_end,
                i + 1,
                overlap_start
            );
        }
    }

    #[test]
    fn chunks_cover_entire_text() {
        let text = "Hello world. This is a test. ".repeat(200);
        let chunks = chunk_text(&text, 200, 0.10);

        // First chunk starts at 0
        assert_eq!(chunks[0].start_byte, 0);
        // Last chunk ends at text length
        assert_eq!(chunks.last().unwrap().end_byte, text.len());
    }

    #[test]
    fn prefers_paragraph_boundary() {
        let text = format!("{}.\n\n{}", "a".repeat(1800), "b".repeat(500));
        let chunks = chunk_text(&text, 2000, 0.10);
        // First chunk should end at the paragraph break
        assert!(chunks[0].text.ends_with('\n'));
    }

    #[test]
    fn chunk_indices_are_sequential() {
        let text = "word ".repeat(1000);
        let chunks = chunk_text(&text, 500, 0.10);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
        }
    }

    #[test]
    fn multibyte_chars_at_chunk_boundary_do_not_panic() {
        // Build text that has multi-byte UTF-8 chars right around the chunk boundary.
        // '─' is 3 bytes (U+2500), so placing it near byte 2000 would previously panic.
        let prefix = "x".repeat(1998);
        let text = format!("{prefix}─── more text after the box-drawing chars ───");
        // This should not panic
        let chunks = chunk_text(&text, 2000, 0.10);
        assert!(!chunks.is_empty());
        // Every chunk must be valid UTF-8 (guaranteed by String, but verify boundaries)
        for chunk in &chunks {
            assert!(text.is_char_boundary(chunk.start_byte));
            assert!(text.is_char_boundary(chunk.end_byte));
        }
    }

    #[test]
    fn multibyte_chars_scattered_throughout_do_not_panic() {
        // Simulate a shell script with box-drawing chars scattered at various offsets.
        // This triggers the find_split_point path with multi-byte chars in the
        // overlap search region.
        let mut text = String::new();
        for i in 0..100 {
            text.push_str(&format!("line {i}: some normal text here\n"));
            if i % 10 == 0 {
                text.push_str("─────────────────────────────────\n");
            }
        }
        let chunks = chunk_text(&text, 500, 0.10);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(text.is_char_boundary(chunk.start_byte));
            assert!(text.is_char_boundary(chunk.end_byte));
        }
    }

    #[test]
    fn multibyte_only_text_does_not_panic() {
        // Text made entirely of multi-byte characters
        let text = "─".repeat(1000); // 3 bytes each = 3000 bytes
        let chunks = chunk_text(&text, 2000, 0.10);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(text.is_char_boundary(chunk.start_byte));
            assert!(text.is_char_boundary(chunk.end_byte));
        }
    }
}
