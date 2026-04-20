//! Config schema types — parsed from plugin's Zod schema serialization.
//!
//! The host reads these to render config forms in the desktop UI.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Serialized config schema from a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(default)]
    pub properties: HashMap<String, FieldSchema>,
    #[serde(default)]
    pub required: Vec<String>,
}

/// Schema for a single config field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<FieldSchema>>,
    /// Hivemind-specific UI metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hivemind: Option<FieldMeta>,
}

/// UI metadata for a config field (label, section, secret, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radio: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl ConfigSchema {
    /// Parse a config schema from a JSON value (as returned by plugin/configSchema).
    pub fn from_value(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }

    /// Get all fields grouped by section.
    pub fn fields_by_section(&self) -> Vec<(String, Vec<(&str, &FieldSchema)>)> {
        let mut sections: Vec<(String, Vec<(&str, &FieldSchema)>)> = Vec::new();
        let mut section_map: HashMap<String, usize> = HashMap::new();

        for (name, field) in &self.properties {
            let section_name = field
                .hivemind
                .as_ref()
                .and_then(|m| m.section.as_deref())
                .unwrap_or("General")
                .to_string();

            let idx = if let Some(&i) = section_map.get(&section_name) {
                i
            } else {
                let i = sections.len();
                section_map.insert(section_name.clone(), i);
                sections.push((section_name, Vec::new()));
                i
            };

            sections[idx].1.push((name.as_str(), field));
        }

        sections
    }

    /// Get secret field names (fields that should be stored in the keyring).
    pub fn secret_fields(&self) -> Vec<&str> {
        self.properties
            .iter()
            .filter(|(_, field)| {
                field
                    .hivemind
                    .as_ref()
                    .and_then(|m| m.secret)
                    .unwrap_or(false)
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config_schema() {
        let json = serde_json::json!({
            "type": "object",
            "properties": {
                "apiKey": {
                    "type": "string",
                    "hivemind": { "label": "API Key", "secret": true, "section": "Auth" }
                },
                "pollInterval": {
                    "type": "number",
                    "default": 60,
                    "minimum": 10,
                    "maximum": 3600,
                    "hivemind": { "label": "Poll Interval", "section": "Sync" }
                }
            },
            "required": ["apiKey"]
        });

        let schema = ConfigSchema::from_value(json).unwrap();
        assert_eq!(schema.properties.len(), 2);
        assert_eq!(schema.required, vec!["apiKey"]);
        assert_eq!(schema.secret_fields(), vec!["apiKey"]);

        let sections = schema.fields_by_section();
        assert_eq!(sections.len(), 2);
    }
}
