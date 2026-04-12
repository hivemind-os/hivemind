#[cfg(test)]
mod tests {
    use hive_classification::model::DataClass;
    use hive_classification::{gate, ChannelClass, GateDecision, OverridePolicy};

    #[test]
    fn test_public_data_always_allowed() {
        // Public data should always be allowed regardless of channel
        let policy = OverridePolicy::default();

        let result = gate(DataClass::Public, ChannelClass::Public, &policy);
        assert!(matches!(result, GateDecision::Allow));

        let result = gate(DataClass::Public, ChannelClass::Internal, &policy);
        assert!(matches!(result, GateDecision::Allow));
    }

    #[test]
    fn test_empty_content_classification() {
        use hive_classification::{LabelContext, LabellerPipeline};

        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify("", &LabelContext::default());

        // Empty content should not trigger any patterns
        println!("Empty content level: {}", result.label.level);
        assert_eq!(result.label.level, DataClass::Internal); // Default
        assert!(result.spans.is_empty());
    }

    #[test]
    fn test_whitespace_only_classification() {
        use hive_classification::{LabelContext, LabellerPipeline};

        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify("   \n\t  ", &LabelContext::default());

        println!("Whitespace-only level: {}", result.label.level);
        assert_eq!(result.label.level, DataClass::Internal); // Default
        assert!(result.spans.is_empty());
    }

    #[test]
    fn test_very_long_content() {
        use hive_classification::{LabelContext, LabellerPipeline};

        let pipeline = LabellerPipeline::default();
        let long_content = "a".repeat(1_000_000); // 1MB of 'a's
        let result = pipeline.classify(&long_content, &LabelContext::default());

        assert_eq!(result.label.level, DataClass::Internal);
        assert!(result.spans.is_empty());
    }

    #[test]
    fn test_multiple_sensitive_patterns() {
        use hive_classification::{LabelContext, LabellerPipeline};

        let pipeline = LabellerPipeline::default();
        let content = "Email: user@example.com and token sk-abc12345678901234567890";
        let result = pipeline.classify(content, &LabelContext::default());

        // Should detect both and pick highest level (Restricted)
        assert_eq!(result.label.level, DataClass::Restricted);
        assert!(result.spans.len() >= 2); // Should find both
    }
}
