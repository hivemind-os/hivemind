use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use regex::Regex;
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

pub struct RegexTool {
    definition: ToolDefinition,
}

impl Default for RegexTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.regex".to_string(),
                name: "Regex".to_string(),
                description: "Search text using a regular expression pattern and return all matches with capture groups.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "description": "Regular expression pattern to match.",
                            "type": "string"
                        },
                        "text": {
                            "description": "Text to search.",
                            "type": "string"
                        },
                        "global": {
                            "description": "If true (default), return all matches. If false, return only the first match.",
                            "type": "boolean",
                            "default": true
                        }
                    },
                    "required": ["pattern", "text"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "matches": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "full": { "type": "string", "description": "Full matched text." },
                                    "groups": {
                                        "type": "array",
                                        "items": { "type": ["string", "null"] },
                                        "description": "Captured groups (excluding the full match)."
                                    },
                                    "start": { "type": "number", "description": "Start byte offset." },
                                    "end": { "type": "number", "description": "End byte offset." }
                                }
                            }
                        },
                        "count": { "type": "number", "description": "Number of matches." }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Regex".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for RegexTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `pattern`".to_string())
            })?;

            let text = input.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `text`".to_string())
            })?;

            let global = input.get("global").and_then(|v| v.as_bool()).unwrap_or(true);

            let re = Regex::new(pattern)
                .map_err(|e| ToolError::InvalidInput(format!("invalid regex pattern: {e}")))?;

            let mut matches = Vec::new();

            for cap in re.captures_iter(text) {
                let full_match = cap.get(0).unwrap();
                let groups: Vec<Value> = (1..cap.len())
                    .map(|i| match cap.get(i) {
                        Some(m) => Value::String(m.as_str().to_string()),
                        None => Value::Null,
                    })
                    .collect();

                matches.push(json!({
                    "full": full_match.as_str(),
                    "groups": groups,
                    "start": full_match.start(),
                    "end": full_match.end(),
                }));

                if !global {
                    break;
                }
            }

            let count = matches.len();
            Ok(ToolResult {
                output: json!({ "matches": matches, "count": count }),
                data_class: DataClass::Internal,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_pattern_matching() {
        let tool = RegexTool::default();
        let result =
            tool.execute(json!({ "pattern": r"\d+", "text": "abc 123 def 456" })).await.unwrap();
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(result.output["count"], 2);
        assert_eq!(matches[0]["full"], "123");
        assert_eq!(matches[1]["full"], "456");
    }

    #[tokio::test]
    async fn test_capture_groups() {
        let tool = RegexTool::default();
        let result = tool
            .execute(json!({
                "pattern": r"(\w+)@(\w+\.\w+)",
                "text": "user@example.com and admin@test.org"
            }))
            .await
            .unwrap();
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(result.output["count"], 2);
        assert_eq!(matches[0]["full"], "user@example.com");
        assert_eq!(matches[0]["groups"][0], "user");
        assert_eq!(matches[0]["groups"][1], "example.com");
        assert_eq!(matches[1]["full"], "admin@test.org");
        assert_eq!(matches[1]["groups"][0], "admin");
        assert_eq!(matches[1]["groups"][1], "test.org");
    }

    #[tokio::test]
    async fn test_global_false_returns_first_match_only() {
        let tool = RegexTool::default();
        let result = tool
            .execute(json!({ "pattern": r"\d+", "text": "10 20 30", "global": false }))
            .await
            .unwrap();
        assert_eq!(result.output["count"], 1);
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["full"], "10");
    }

    #[tokio::test]
    async fn test_invalid_regex_returns_error() {
        let tool = RegexTool::default();
        let result = tool.execute(json!({ "pattern": r"[invalid", "text": "hello" })).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("invalid regex pattern"),
            "expected invalid regex error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_no_matches_returns_empty() {
        let tool = RegexTool::default();
        let result =
            tool.execute(json!({ "pattern": r"\d+", "text": "no numbers here" })).await.unwrap();
        assert_eq!(result.output["count"], 0);
        let matches = result.output["matches"].as_array().unwrap();
        assert!(matches.is_empty());
    }
}
