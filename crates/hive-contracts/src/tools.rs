use hive_classification::ChannelClass;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolApproval {
    Auto,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolAnnotations {
    pub title: String,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub channel_class: ChannelClass,
    pub side_effects: bool,
    pub approval: ToolApproval,
    pub annotations: ToolAnnotations,
}

pub struct ToolDefinitionBuilder {
    id: String,
    name: String,
    description: String,
    input_schema: Value,
    output_schema: Option<Value>,
    channel_class: ChannelClass,
    side_effects: bool,
    approval: ToolApproval,
    annotations: ToolAnnotations,
}

impl ToolDefinitionBuilder {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let name_str: String = name.into();
        Self {
            id: id.into(),
            name: name_str.clone(),
            description: String::new(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: None,
            channel_class: ChannelClass::Internal,
            side_effects: false,
            approval: ToolApproval::Auto,
            annotations: ToolAnnotations {
                title: name_str,
                read_only_hint: None,
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: None,
            },
        }
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn channel_class(mut self, class: ChannelClass) -> Self {
        self.channel_class = class;
        self
    }

    pub fn side_effects(mut self, has: bool) -> Self {
        self.side_effects = has;
        self
    }

    pub fn approval(mut self, approval: ToolApproval) -> Self {
        self.approval = approval;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.annotations.read_only_hint = Some(true);
        self.annotations.destructive_hint = Some(false);
        self.side_effects = false;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.annotations.destructive_hint = Some(true);
        self.annotations.read_only_hint = Some(false);
        self.side_effects = true;
        self
    }

    pub fn idempotent(mut self) -> Self {
        self.annotations.idempotent_hint = Some(true);
        self
    }

    pub fn open_world(mut self) -> Self {
        self.annotations.open_world_hint = Some(true);
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.annotations.title = title.into();
        self
    }

    pub fn build(self) -> ToolDefinition {
        ToolDefinition {
            id: self.id,
            name: self.name,
            description: self.description,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            channel_class: self.channel_class,
            side_effects: self.side_effects,
            approval: self.approval,
            annotations: self.annotations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let def =
            ToolDefinitionBuilder::new("test.tool", "Test Tool").description("A test tool").build();
        assert_eq!(def.id, "test.tool");
        assert_eq!(def.name, "Test Tool");
        assert_eq!(def.description, "A test tool");
        assert_eq!(def.channel_class, ChannelClass::Internal);
        assert_eq!(def.approval, ToolApproval::Auto);
        assert!(!def.side_effects);
        assert_eq!(def.annotations.title, "Test Tool");
    }

    #[test]
    fn builder_read_only_sets_annotations() {
        let def = ToolDefinitionBuilder::new("test.read", "Read Tool").read_only().build();
        assert_eq!(def.annotations.read_only_hint, Some(true));
        assert_eq!(def.annotations.destructive_hint, Some(false));
        assert!(!def.side_effects);
    }

    #[test]
    fn builder_destructive_sets_annotations() {
        let def = ToolDefinitionBuilder::new("test.delete", "Delete Tool")
            .destructive()
            .approval(ToolApproval::Ask)
            .build();
        assert_eq!(def.annotations.destructive_hint, Some(true));
        assert_eq!(def.annotations.read_only_hint, Some(false));
        assert!(def.side_effects);
        assert_eq!(def.approval, ToolApproval::Ask);
    }
}
