use crate::model::SensitiveSpan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionResult {
    pub content: String,
    pub spans: Vec<SensitiveSpan>,
}

pub fn redact(content: &str, spans: &[SensitiveSpan]) -> RedactionResult {
    if spans.is_empty() {
        return RedactionResult { content: content.to_string(), spans: Vec::new() };
    }

    let mut spans = spans.to_vec();
    spans.sort_by_key(|span| span.start);

    // Clamp span boundaries to valid UTF-8 char boundaries
    for span in &mut spans {
        span.start = snap_to_char_boundary(content, span.start);
        span.end = snap_to_char_boundary(content, span.end);
    }

    let mut merged: Vec<SensitiveSpan> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut() {
            if span.start < last.end {
                last.end = last.end.max(span.end);
                continue;
            }
        }

        merged.push(span);
    }

    let mut result = String::with_capacity(content.len());
    let mut cursor = 0;
    for span in &merged {
        if span.start > cursor {
            result.push_str(&content[cursor..span.start]);
        }
        result.push_str("[REDACTED]");
        cursor = span.end;
    }

    if cursor < content.len() {
        result.push_str(&content[cursor..]);
    }

    RedactionResult { content: result, spans: merged }
}

/// Snap a byte position to the nearest valid UTF-8 character boundary,
/// rounding down. Returns `content.len()` if `pos` exceeds the length.
fn snap_to_char_boundary(content: &str, pos: usize) -> usize {
    if pos >= content.len() {
        return content.len();
    }
    // Walk backwards to find the nearest char boundary
    let mut p = pos;
    while !content.is_char_boundary(p) && p > 0 {
        p -= 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DataClass;

    #[test]
    fn redacts_sensitive_spans() {
        let text = "token sk-abc12345678901234567890 should not leak";
        let start = text.find("sk-").expect("token start");
        let end = start + "sk-abc12345678901234567890".len();
        let result = redact(
            text,
            &[SensitiveSpan {
                start,
                end,
                reason: "openai-api-key".to_string(),
                level: DataClass::Restricted,
            }],
        );

        assert_eq!(result.content, "token [REDACTED] should not leak");
    }
}
