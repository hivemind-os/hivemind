#[cfg(test)]
mod tests {
    use hive_classification::model::DataClass;
    use hive_classification::{LabelContext, LabellerPipeline, SourceKind};

    #[test]
    fn test_source_kind_web_gives_public() {
        // Web content should be Public
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "some content",
            &LabelContext { source_path: None, source_kind: SourceKind::Web, source_name: None },
        );
        assert_eq!(result.label.level, DataClass::Public);
    }

    #[test]
    fn test_pattern_overrides_source_when_higher() {
        // If pattern finds restricted data, it should override source classification
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "data from web sk-abc12345678901234567890",
            &LabelContext {
                source_path: None,
                source_kind: SourceKind::Web, // Would be Public
                source_name: None,
            },
        );
        // Pattern finds Restricted token, should override Public
        assert_eq!(result.label.level, DataClass::Restricted);
    }

    #[test]
    fn test_filesystem_without_sensitive_path_gives_none() {
        // FileSystem source without sensitive path should not add a label
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "regular content",
            &LabelContext {
                source_path: Some("/home/user/document.txt".to_string()),
                source_kind: SourceKind::FileSystem,
                source_name: None,
            },
        );
        // Should default to Internal (no source label applied)
        assert_eq!(result.label.level, DataClass::Internal);
    }

    #[test]
    fn test_unknown_source_with_no_patterns() {
        // Unknown source with no patterns should default to Internal
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "some text",
            &LabelContext {
                source_path: None,
                source_kind: SourceKind::Unknown,
                source_name: None,
            },
        );
        assert_eq!(result.label.level, DataClass::Internal);
    }
}
