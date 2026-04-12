use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Meta-tool that lets models discover full tool definitions on demand.
///
/// Instead of injecting every tool's full schema into the system prompt
/// (which overwhelms small local models), this tool allows the model to
/// request details about specific tools by ID when it actually needs them.
pub struct DiscoverToolsTool {
    definition: ToolDefinition,
    /// Snapshot of all other tool definitions, taken at registration time.
    catalog: Vec<ToolDefinition>,
}

impl DiscoverToolsTool {
    /// Create a new `DiscoverToolsTool` with a snapshot of tool definitions.
    pub fn new(catalog: Vec<ToolDefinition>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.discover_tools".to_string(),
                name: "Discover Tools".to_string(),
                description: concat!(
                    "Get full details (description and parameter schema) for one or more tools by ID. ",
                    "Call this before using a tool for the first time so you know the correct parameters. ",
                    "Pass an array of tool IDs to get their full definitions.",
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "tool_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of tool IDs to get full details for"
                        }
                    },
                    "required": ["tool_ids"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Discover Tools".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            catalog,
        }
    }
}

impl Tool for DiscoverToolsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let tool_ids: Vec<String> = match input.get("tool_ids") {
                Some(ids) => serde_json::from_value(ids.clone()).unwrap_or_default(),
                None => Vec::new(),
            };

            if tool_ids.is_empty() {
                // Return the compact catalog (names + descriptions only)
                let catalog: Vec<Value> = self
                    .catalog
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "description": t.description,
                        })
                    })
                    .collect();
                return Ok(ToolResult {
                    output: json!({ "tools": catalog }),
                    data_class: DataClass::Internal,
                });
            }

            // Return full definitions for requested tools
            let matched: Vec<Value> = self
                .catalog
                .iter()
                .filter(|t| tool_ids.iter().any(|id| id == &t.id))
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();

            let not_found: Vec<&str> = tool_ids
                .iter()
                .filter(|id| !self.catalog.iter().any(|t| &t.id == *id))
                .map(|s| s.as_str())
                .collect();

            let mut result = json!({ "tools": matched });
            if !not_found.is_empty() {
                result["not_found"] = json!(not_found);
            }

            Ok(ToolResult { output: result, data_class: DataClass::Internal })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::ToolDefinitionBuilder;

    fn sample_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinitionBuilder::new("filesystem.read", "Read File")
                .description("Read a text file from the workspace")
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" }
                    },
                    "required": ["path"]
                }))
                .build(),
            ToolDefinitionBuilder::new("filesystem.write", "Write File")
                .description("Write to a file in the workspace")
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }))
                .build(),
        ]
    }

    #[tokio::test]
    async fn empty_ids_returns_catalog() {
        let tool = DiscoverToolsTool::new(sample_tools());
        let result = tool.execute(json!({"tool_ids": []})).await.unwrap();
        let tools = result.output["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        // Catalog entries should have id + description but NOT parameters
        assert!(tools[0].get("id").is_some());
        assert!(tools[0].get("description").is_some());
        assert!(tools[0].get("parameters").is_none());
    }

    #[tokio::test]
    async fn specific_ids_return_full_definitions() {
        let tool = DiscoverToolsTool::new(sample_tools());
        let result = tool.execute(json!({"tool_ids": ["filesystem.read"]})).await.unwrap();
        let tools = result.output["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["id"], "filesystem.read");
        // Full definition should include parameters
        assert!(tools[0].get("parameters").is_some());
    }

    #[tokio::test]
    async fn unknown_ids_reported() {
        let tool = DiscoverToolsTool::new(sample_tools());
        let result = tool.execute(json!({"tool_ids": ["nonexistent.tool"]})).await.unwrap();
        let not_found = result.output["not_found"].as_array().unwrap();
        assert_eq!(not_found.len(), 1);
        assert_eq!(not_found[0], "nonexistent.tool");
    }
}
