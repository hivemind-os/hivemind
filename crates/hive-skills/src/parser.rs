//! SKILL.md parser — extracts YAML frontmatter + markdown body.

use hive_contracts::SkillManifest;
use std::collections::BTreeMap;

/// A parsed SKILL.md file.
#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub manifest: SkillManifest,
    pub body: String,
}

/// Parse a SKILL.md file content into manifest + body.
///
/// Follows the Agent Skills spec with lenient validation:
/// - Warns on issues but loads when possible
/// - Returns Err only if YAML is completely unparseable or description is missing
pub fn parse_skill_md(content: &str) -> Result<ParsedSkill, ParseError> {
    let content = content.trim();

    // Find frontmatter delimiters
    if !content.starts_with("---") {
        return Err(ParseError::MissingFrontmatter);
    }

    let after_opening = &content[3..];
    let closing_pos = after_opening.find("\n---").ok_or(ParseError::MissingFrontmatter)?;

    let yaml_block = &after_opening[..closing_pos].trim();
    let body_start = 3 + closing_pos + 4; // "---" + position + "\n---"
    let body = if body_start < content.len() {
        content[body_start..].trim().to_string()
    } else {
        String::new()
    };

    // Try parsing YAML. Handle common issues like unquoted colons.
    let raw: BTreeMap<String, serde_yaml::Value> = match serde_yaml::from_str(yaml_block) {
        Ok(v) => v,
        Err(_) => {
            // Fallback: try quoting values that contain colons
            let fixed = fix_unquoted_colons(yaml_block);
            serde_yaml::from_str(&fixed).map_err(|e| ParseError::InvalidYaml(e.to_string()))?
        }
    };

    let name = raw.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default();

    let description = raw
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(ParseError::MissingDescription)?;

    if description.is_empty() {
        return Err(ParseError::MissingDescription);
    }

    let license = raw.get("license").and_then(|v| v.as_str()).map(|s| s.to_string());
    let compatibility = raw.get("compatibility").and_then(|v| v.as_str()).map(|s| s.to_string());
    let allowed_tools = raw
        .get("allowed-tools")
        .or_else(|| raw.get("allowed_tools"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let metadata = raw
        .get("metadata")
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str()?.to_string();
                    let val = v.as_str()?.to_string();
                    Some((key, val))
                })
                .collect()
        })
        .unwrap_or_default();

    // Validate name (lenient: warn but don't fail)
    if !name.is_empty() {
        if name.len() > 64 {
            tracing::warn!("Skill name '{}' exceeds 64 characters", name);
        }
        if name.starts_with('-') || name.ends_with('-') {
            tracing::warn!("Skill name '{}' starts or ends with a hyphen", name);
        }
        if name.contains("--") {
            tracing::warn!("Skill name '{}' contains consecutive hyphens", name);
        }
    }

    if description.len() > 1024 {
        tracing::warn!(
            "Skill '{}' description exceeds 1024 characters ({})",
            name,
            description.len()
        );
    }

    Ok(ParsedSkill {
        manifest: SkillManifest {
            name,
            description,
            license,
            compatibility,
            metadata,
            allowed_tools,
        },
        body,
    })
}

/// Attempt to fix unquoted YAML values that contain colons.
fn fix_unquoted_colons(yaml: &str) -> String {
    yaml.lines()
        .map(|line| {
            if let Some(colon_pos) = line.find(':') {
                let key = &line[..colon_pos];
                let value = line[colon_pos + 1..].trim();
                // If the value contains a colon and isn't already quoted, wrap it
                if value.contains(':') && !value.starts_with('"') && !value.starts_with('\'') {
                    return format!("{}: \"{}\"", key, value.replace('"', "\\\""));
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("missing YAML frontmatter (file must start with --- and have a closing ---)")]
    MissingFrontmatter,
    #[error("invalid YAML frontmatter: {0}")]
    InvalidYaml(String),
    #[error("missing required 'description' field in frontmatter")]
    MissingDescription,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_skill() {
        let content = r#"---
name: test-skill
description: A test skill for unit testing.
---
# Test Skill

Do the thing.
"#;
        let parsed = parse_skill_md(content).unwrap();
        assert_eq!(parsed.manifest.name, "test-skill");
        assert_eq!(parsed.manifest.description, "A test skill for unit testing.");
        assert!(parsed.body.starts_with("# Test Skill"));
        assert!(parsed.body.contains("Do the thing."));
    }

    #[test]
    fn parse_full_frontmatter() {
        let content = r#"---
name: pdf-processing
description: Extract text from PDFs, fill forms, and merge files.
license: Apache-2.0
compatibility: Requires Python 3.9+
allowed-tools: filesystem.read filesystem.write shell.execute
metadata:
  author: anthropic
  version: "1.0"
---
# PDF Processing

Use this skill when the user needs to work with PDF files.
"#;
        let parsed = parse_skill_md(content).unwrap();
        assert_eq!(parsed.manifest.name, "pdf-processing");
        assert_eq!(parsed.manifest.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(parsed.manifest.compatibility.as_deref(), Some("Requires Python 3.9+"));
        assert_eq!(
            parsed.manifest.allowed_tools.as_deref(),
            Some("filesystem.read filesystem.write shell.execute")
        );
        assert_eq!(parsed.manifest.metadata.get("author").map(|s| s.as_str()), Some("anthropic"));
        assert_eq!(parsed.manifest.metadata.get("version").map(|s| s.as_str()), Some("1.0"));
    }

    #[test]
    fn parse_unquoted_colon_in_description() {
        let content = r#"---
name: colon-test
description: Use this skill when: the user asks about PDFs
---
Body content.
"#;
        let parsed = parse_skill_md(content).unwrap();
        assert!(parsed.manifest.description.contains("when"));
    }

    #[test]
    fn missing_description_fails() {
        let content = r#"---
name: no-desc
---
Body.
"#;
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn missing_frontmatter_fails() {
        let content = "# Just Markdown\n\nNo frontmatter here.";
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn empty_body_ok() {
        let content = r#"---
name: empty-body
description: A skill with no body content.
---"#;
        let parsed = parse_skill_md(content).unwrap();
        assert_eq!(parsed.body, "");
    }
}
