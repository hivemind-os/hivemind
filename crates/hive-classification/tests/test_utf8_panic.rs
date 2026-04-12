#[cfg(test)]
mod tests {
    use hive_classification::model::{DataClass, SensitiveSpan};
    use hive_classification::redact;

    #[test]
    fn test_invalid_utf8_boundary_snaps_safely() {
        let text = "Start 🔐secret here";

        // Find the emoji - it starts at byte 6 and is 4 bytes long (bytes 6-10)
        let emoji_pos = text.find("🔐").unwrap();

        // Create a span that lands mid-emoji (invalid UTF-8 boundary)
        let invalid_spans = vec![SensitiveSpan {
            start: emoji_pos + 2, // Mid-emoji!
            end: emoji_pos + 4,
            reason: "test".to_string(),
            level: DataClass::Restricted,
        }];

        // snap_to_char_boundary should prevent panics
        let result = redact(text, &invalid_spans);
        // Should produce a valid string (not panic)
        assert!(result.content.is_char_boundary(0));
    }
}
