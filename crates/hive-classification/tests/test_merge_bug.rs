#[cfg(test)]
mod tests {
    use hive_classification::model::{DataClass, SensitiveSpan};
    use hive_classification::redact;

    #[test]
    fn test_adjacent_spans_merge_issue() {
        let text = "hello world";

        // Two adjacent non-overlapping spans
        let spans = vec![
            SensitiveSpan {
                start: 0,
                end: 5, // "hello"
                reason: "first".to_string(),
                level: DataClass::Restricted,
            },
            SensitiveSpan {
                start: 5,
                end: 10, // " worl"
                reason: "second".to_string(),
                level: DataClass::Restricted,
            },
        ];

        let result = redact(text, &spans);
        println!("Original: '{text}'");
        println!("Result:   '{}'", result.content);
        println!("Spans merged: {:?}", result.spans);

        // With < (strict overlap check), adjacent non-overlapping spans
        // [0..5) and [5..10) are redacted separately
        assert_eq!(result.content, "[REDACTED][REDACTED]d");
        assert_eq!(result.spans.len(), 2, "adjacent spans should not merge");
    }
}
