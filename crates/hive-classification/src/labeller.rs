use crate::model::{ClassificationLabel, DataClass, LabelSource, SensitiveSpan};
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SourceKind {
    FileSystem,
    Clipboard,
    Web,
    Mpc,
    ToolResult,
    Messaging,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelContext {
    pub source_path: Option<String>,
    pub source_kind: SourceKind,
    pub source_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub label: ClassificationLabel,
    pub spans: Vec<SensitiveSpan>,
}

pub trait Labeller {
    fn classify(&self, content: &str, context: &LabelContext) -> Option<Detection>;
}

#[derive(Debug, Clone)]
pub struct PatternLabeller {
    rules: Vec<PatternRule>,
}

#[derive(Debug, Clone)]
struct PatternRule {
    name: &'static str,
    regex: Regex,
    level: DataClass,
}

impl Default for PatternLabeller {
    fn default() -> Self {
        Self {
            rules: vec![
                PatternRule {
                    name: "openai-api-key",
                    regex: Regex::new(r"sk-[A-Za-z0-9]{20,}").expect("valid regex"),
                    level: DataClass::Restricted,
                },
                PatternRule {
                    name: "github-token",
                    regex: Regex::new(r"gh[pousr]_[A-Za-z0-9]{20,}").expect("valid regex"),
                    level: DataClass::Restricted,
                },
                PatternRule {
                    name: "private-key",
                    regex: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").expect("valid regex"),
                    level: DataClass::Restricted,
                },
                PatternRule {
                    name: "email-address",
                    regex: Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
                        .expect("valid regex"),
                    level: DataClass::Confidential,
                },
            ],
        }
    }
}

impl Labeller for PatternLabeller {
    fn classify(&self, content: &str, _context: &LabelContext) -> Option<Detection> {
        let mut strongest_level: Option<DataClass> = None;
        let mut reason: Option<String> = None;
        let mut spans = Vec::new();

        for rule in &self.rules {
            for hit in rule.regex.find_iter(content) {
                spans.push(SensitiveSpan {
                    start: hit.start(),
                    end: hit.end(),
                    reason: rule.name.to_string(),
                    level: rule.level,
                });

                if strongest_level.is_none_or(|current| rule.level > current) {
                    strongest_level = Some(rule.level);
                    reason = Some(format!("matched sensitive pattern {}", rule.name));
                }
            }
        }

        strongest_level.map(|level| Detection {
            label: ClassificationLabel::new(level, LabelSource::Pattern, reason),
            spans,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceLabeller;

impl Labeller for SourceLabeller {
    fn classify(&self, _content: &str, context: &LabelContext) -> Option<Detection> {
        if let Some(path) = &context.source_path {
            let lower = path.to_ascii_lowercase();
            if lower.contains(".ssh")
                || lower.contains(".env")
                || lower.contains("secret")
                || lower.contains("credential")
            {
                return Some(Detection {
                    label: ClassificationLabel::new(
                        DataClass::Restricted,
                        LabelSource::Source,
                        Some(format!("source path {path} is sensitive")),
                    ),
                    spans: Vec::new(),
                });
            }
        }

        let (level, reason) = match context.source_kind {
            SourceKind::Web => {
                (DataClass::Public, "content originated from a public web source".to_string())
            }
            SourceKind::Clipboard | SourceKind::Messaging => (
                DataClass::Internal,
                "content originated from a user-controlled source".to_string(),
            ),
            SourceKind::Mpc | SourceKind::ToolResult => {
                (DataClass::Internal, "tool and MCP outputs default to internal".to_string())
            }
            SourceKind::FileSystem | SourceKind::Unknown => return None,
        };

        Some(Detection {
            label: ClassificationLabel::new(level, LabelSource::Source, Some(reason)),
            spans: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationResult {
    pub label: ClassificationLabel,
    pub spans: Vec<SensitiveSpan>,
}

#[derive(Debug, Clone, Default)]
pub struct LabellerPipeline {
    pattern: PatternLabeller,
    source: SourceLabeller,
}

impl LabellerPipeline {
    pub fn classify(&self, content: &str, context: &LabelContext) -> ClassificationResult {
        let pattern_detection = self.pattern.classify(content, context);
        let source_detection = self.source.classify(content, context);

        let mut spans = Vec::new();
        let mut strongest = source_detection.as_ref().map(|d| d.label.clone());

        if let Some(source_detection) = source_detection {
            spans.extend(source_detection.spans);
            strongest = Some(source_detection.label);
        }

        if let Some(pattern_detection) = pattern_detection {
            spans.extend(pattern_detection.spans);
            strongest = Some(match strongest {
                Some(current) if current.level >= pattern_detection.label.level => current,
                _ => pattern_detection.label,
            });
        }

        ClassificationResult {
            label: strongest.unwrap_or_else(|| {
                ClassificationLabel::new(
                    DataClass::Internal,
                    LabelSource::Source,
                    Some("default internal policy".to_string()),
                )
            }),
            spans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sensitive_patterns() {
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "contact ops@example.com and rotate sk-abc12345678901234567890",
            &LabelContext::default(),
        );

        assert_eq!(result.label.level, DataClass::Restricted);
        assert_eq!(result.spans.len(), 2);
    }

    #[test]
    fn source_path_marks_sensitive_files_as_restricted() {
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify(
            "ssh key data",
            &LabelContext {
                source_path: Some("C:\\Users\\me\\.ssh\\id_ed25519".to_string()),
                source_kind: SourceKind::FileSystem,
                source_name: None,
            },
        );

        assert_eq!(result.label.level, DataClass::Restricted);
    }

    #[test]
    fn falls_back_to_internal_when_nothing_matches() {
        let pipeline = LabellerPipeline::default();
        let result = pipeline.classify("hello world", &LabelContext::default());

        assert_eq!(result.label.level, DataClass::Internal);
        assert!(result.spans.is_empty());
    }
}
