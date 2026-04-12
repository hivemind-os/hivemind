use anyhow::{bail, Context, Result};
use hive_contracts::{
    HiveMindConfig, HiveMindPaths, McpServerConfig, McpTransportConfig, ModelProviderConfig,
    Persona, ProviderKindConfig, DEFAULT_CONFIG_FILE,
};
use serde_yaml::{Mapping, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn load_config() -> Result<HiveMindConfig> {
    let cwd = env::current_dir().context("failed to determine the current working directory")?;
    load_config_with_cwd(&cwd)
}

pub fn load_config_with_cwd(cwd: &Path) -> Result<HiveMindConfig> {
    let paths = discover_paths()?;
    let project_config = cwd.join(".hivemind").join(DEFAULT_CONFIG_FILE);
    load_config_from_paths(Some(&paths.config_path), Some(&project_config))
}

pub fn load_config_from_paths(
    user_config: Option<&Path>,
    project_config: Option<&Path>,
) -> Result<HiveMindConfig> {
    let mut merged = serde_yaml::to_value(HiveMindConfig::default())
        .context("failed to convert default config into mergeable yaml")?;

    for path in [user_config, project_config].into_iter().flatten() {
        if !path.exists() {
            continue;
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;

        if contents.trim().is_empty() {
            continue;
        }

        let overlay: Value = serde_yaml::from_str(&contents)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        merge_yaml_values(&mut merged, overlay);
    }

    let mut config: HiveMindConfig =
        serde_yaml::from_value(merged).context("failed to deserialize merged hivemind config")?;

    // Migrate legacy provider-level capabilities → per-model capabilities
    for provider in &mut config.models.providers {
        provider.migrate_capabilities();
    }

    // Ensure a local-models provider always exists
    if !config.models.providers.iter().any(|p| p.kind == ProviderKindConfig::LocalModels) {
        config.models.providers.push(ModelProviderConfig {
            id: "local".to_string(),
            name: Some("Local Models".to_string()),
            kind: ProviderKindConfig::LocalModels,
            base_url: None,
            auth: hive_contracts::ProviderAuthConfig::None,
            models: vec![],
            capabilities: std::collections::BTreeSet::new(),
            model_capabilities: std::collections::BTreeMap::new(),
            channel_class: hive_contracts::ChannelClass::LocalOnly,
            priority: 50,
            enabled: true,
            options: hive_contracts::ProviderOptionsConfig::default(),
        });
    }

    validate_config(&config)?;
    Ok(config)
}

pub fn validate_config_file(path: &Path) -> Result<HiveMindConfig> {
    load_config_from_paths(Some(path), None)
}

pub fn validate_config(config: &HiveMindConfig) -> Result<()> {
    let mut seen_ids = HashSet::new();
    let mut providers_by_id = HashMap::new();

    for provider in &config.models.providers {
        if provider.id.trim().is_empty() {
            bail!("provider ids must not be empty");
        }

        if !seen_ids.insert(provider.id.clone()) {
            bail!("duplicate provider id `{}`", provider.id);
        }

        let display = provider.display_name();

        // LocalModels provider can have empty models (auto-discovered) and
        // empty model_capabilities (populated at runtime).
        if provider.kind == ProviderKindConfig::LocalModels {
            provider.validate_base_url().map_err(|e| anyhow::anyhow!(e))?;
            providers_by_id.insert(provider.id.clone(), provider);
            continue;
        }

        if provider.models.is_empty() {
            bail!("provider `{display}` must declare at least one model");
        }

        if provider.models.iter().any(|model| model.trim().is_empty()) {
            bail!("provider `{display}` contains a blank model identifier");
        }

        provider.validate_base_url().map_err(|e| anyhow::anyhow!(e))?;
        providers_by_id.insert(provider.id.clone(), provider);
    }

    if providers_by_id.is_empty() {
        bail!("at least one provider must be configured");
    }

    // Validate per-persona MCP servers.
    for persona in &config.personas {
        if let Err(e) = validate_mcp_servers(&persona.mcp_servers) {
            bail!("persona `{}` MCP config invalid: {e}", persona.id);
        }
    }

    // Validate threshold ranges.
    if !(0.0..=1.0).contains(&config.security.prompt_injection.confidence_threshold) {
        bail!(
            "security.prompt_injection.confidence_threshold must be between 0.0 and 1.0, got {}",
            config.security.prompt_injection.confidence_threshold
        );
    }
    if !(0.0..=1.0).contains(&config.compaction.trigger_threshold) {
        bail!(
            "compaction.trigger_threshold must be between 0.0 and 1.0, got {}",
            config.compaction.trigger_threshold
        );
    }

    Ok(())
}

pub fn config_to_yaml(config: &HiveMindConfig) -> Result<String> {
    serde_yaml::to_string(config).context("failed to serialize hivemind config as yaml")
}

/// Write a configuration to a YAML file, creating parent directories
/// if needed. The config is validated before writing.
pub fn save_config(config: &HiveMindConfig, path: &Path) -> Result<()> {
    validate_config(config).context("config validation failed before save")?;
    let yaml = config_to_yaml(config)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    fs::write(path, yaml).with_context(|| format!("failed to write config to {}", path.display()))
}

pub fn discover_paths() -> Result<HiveMindPaths> {
    if let Some(config_path) = env::var_os("HIVEMIND_CONFIG_PATH").map(PathBuf::from) {
        let hivemind_home = config_path
            .parent()
            .map(Path::to_path_buf)
            .context("HIVEMIND_CONFIG_PATH must have a parent directory")?;
        return Ok(hivemind_paths_from(hivemind_home, config_path));
    }

    let hivemind_home = env::var_os("HIVEMIND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".hivemind")))
        .context("unable to determine the hivemind home directory")?;

    let config_path = hivemind_home.join(DEFAULT_CONFIG_FILE);
    Ok(hivemind_paths_from(hivemind_home, config_path))
}

pub fn hivemind_paths_from(hivemind_home: PathBuf, config_path: PathBuf) -> HiveMindPaths {
    let run_dir = hivemind_home.join("run");
    let personas_dir = hivemind_home.join("personas");
    let audit_log_path = hivemind_home.join("audit.log");
    let knowledge_graph_path = hivemind_home.join("knowledge.db");
    let risk_ledger_path = hivemind_home.join("risk-ledger.db");
    let local_models_db_path = hivemind_home.join("local-models.db");
    let pid_file_path = run_dir.join("hive-daemon.pid");

    HiveMindPaths {
        hivemind_home,
        config_path,
        personas_dir,
        run_dir,
        audit_log_path,
        knowledge_graph_path,
        risk_ledger_path,
        local_models_db_path,
        pid_file_path,
    }
}

pub fn ensure_paths(paths: &HiveMindPaths) -> Result<()> {
    fs::create_dir_all(&paths.hivemind_home)
        .with_context(|| format!("failed to create {}", paths.hivemind_home.display()))?;
    fs::create_dir_all(&paths.run_dir)
        .with_context(|| format!("failed to create {}", paths.run_dir.display()))?;
    fs::create_dir_all(&paths.personas_dir)
        .with_context(|| format!("failed to create {}", paths.personas_dir.display()))?;
    Ok(())
}

// ── Persona file operations ─────────────────────────────────────────

/// Load all personas by recursively walking the personas directory tree.
///
/// The persona ID is derived from the relative path of its `persona.yaml`
/// file.  For example, `personas/system/general/persona.yaml` yields
/// `id = "system/general"`.
///
/// Personas whose ID matches a bundled (factory-shipped) persona are
/// automatically marked with `bundled = true`.
pub fn load_personas(personas_dir: &Path) -> Result<Vec<Persona>> {
    if !personas_dir.exists() {
        return Ok(Vec::new());
    }

    let mut personas = Vec::new();
    walk_persona_dir(personas_dir, personas_dir, &mut personas)?;
    personas.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(personas)
}

/// Recursively walk the personas directory to find `persona.yaml` files.
fn walk_persona_dir(root: &Path, dir: &Path, personas: &mut Vec<Persona>) -> Result<()> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("failed to read directory {}", dir.display()))?;

    for entry in entries {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();
        if path.is_dir() {
            // Check if this directory contains a persona.yaml
            let persona_file = path.join("persona.yaml");
            if persona_file.exists() {
                let contents = fs::read_to_string(&persona_file)
                    .with_context(|| format!("failed to read {}", persona_file.display()))?;
                if !contents.trim().is_empty() {
                    let mut persona: Persona = serde_yaml::from_str(&contents)
                        .with_context(|| format!("failed to parse {}", persona_file.display()))?;
                    // Derive the ID from the relative path
                    let rel = path.strip_prefix(root).with_context(|| {
                        format!("path {} not under root {}", path.display(), root.display())
                    })?;
                    let id = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join("/");
                    persona.id = id;
                    if crate::bundled::is_bundled_persona(&persona.id) {
                        persona.bundled = true;
                    }
                    personas.push(persona);
                }
            }
            // Continue recursing into subdirectories (there may be nested namespaces)
            walk_persona_dir(root, &path, personas)?;
        }
    }
    Ok(())
}

/// Validate that a persona ID is well-formed for use as a directory path.
fn validate_persona_id(id: &str) -> Result<()> {
    Persona::validate_id(id).map_err(|e| anyhow::anyhow!(e))
}

/// Save a persona to `personas/{id-path}/persona.yaml`, creating
/// intermediate directories as needed.
pub fn save_persona(personas_dir: &Path, persona: &Persona) -> Result<()> {
    validate_persona_id(&persona.id)?;
    let persona_dir = personas_dir.join(persona.id.replace('/', std::path::MAIN_SEPARATOR_STR));
    fs::create_dir_all(&persona_dir)
        .with_context(|| format!("failed to create directory {}", persona_dir.display()))?;
    let file_path = persona_dir.join("persona.yaml");
    let yaml = serde_yaml::to_string(persona)
        .with_context(|| format!("failed to serialize persona '{}'", persona.id))?;
    fs::write(&file_path, yaml)
        .with_context(|| format!("failed to write persona file {}", file_path.display()))
}

/// Archive a persona by setting `archived = true` on its YAML file.
///
/// The directory is kept on disk so that existing workflows can still resolve
/// the persona.  If the persona directory does not exist the call is a no-op.
pub fn archive_persona(personas_dir: &Path, persona_id: &str) -> Result<()> {
    validate_persona_id(persona_id)?;
    let persona_dir = personas_dir.join(persona_id.replace('/', std::path::MAIN_SEPARATOR_STR));
    let file_path = persona_dir.join("persona.yaml");
    if !file_path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&file_path)
        .with_context(|| format!("failed to read persona file {}", file_path.display()))?;
    let mut persona: Persona = serde_yaml::from_str(&contents)
        .with_context(|| format!("failed to parse persona file {}", file_path.display()))?;
    persona.archived = true;
    save_persona(personas_dir, &persona)
}

/// Migrate personas from config.yaml to individual files.
/// Personas already present in the directory are skipped.
pub fn migrate_personas_from_config(personas_dir: &Path, personas: &[Persona]) -> Result<usize> {
    if personas.is_empty() {
        return Ok(0);
    }
    fs::create_dir_all(personas_dir).with_context(|| {
        format!("failed to create personas directory {}", personas_dir.display())
    })?;

    let mut migrated = 0;
    for persona in personas {
        if let Err(e) = Persona::validate_id(&persona.id) {
            tracing::warn!(
                persona_id = %persona.id,
                "skipping persona migration: {e}"
            );
            continue;
        }
        let persona_dir = personas_dir.join(persona.id.replace('/', std::path::MAIN_SEPARATOR_STR));
        let file_path = persona_dir.join("persona.yaml");
        if file_path.exists() {
            continue;
        }
        save_persona(personas_dir, persona)?;
        migrated += 1;
    }
    Ok(migrated)
}

fn validate_mcp_servers(servers: &[McpServerConfig]) -> Result<()> {
    let mut seen = HashSet::new();

    for server in servers {
        if server.id.trim().is_empty() {
            bail!("mcp server ids must not be empty");
        }

        if !seen.insert(server.id.clone()) {
            bail!("duplicate mcp server id `{}`", server.id);
        }

        match server.transport {
            McpTransportConfig::Stdio => {
                if server.command.as_deref().unwrap_or("").trim().is_empty() {
                    bail!("mcp server `{}` must include a command for stdio transport", server.id);
                }
            }
            McpTransportConfig::Sse | McpTransportConfig::StreamableHttp => {
                if server.url.as_deref().unwrap_or("").trim().is_empty() {
                    bail!(
                        "mcp server `{}` must include a url for {} transport",
                        server.id,
                        match server.transport {
                            McpTransportConfig::Sse => "sse",
                            McpTransportConfig::StreamableHttp => "streamable-http",
                            McpTransportConfig::Stdio => "stdio",
                        }
                    );
                }
            }
        }
    }

    Ok(())
}

fn merge_yaml_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Mapping(base_map), Value::Mapping(overlay_map)) => {
            merge_mapping(base_map, overlay_map);
        }
        (base_value, overlay_value) => *base_value = overlay_value,
    }
}

fn merge_mapping(base: &mut Mapping, overlay: Mapping) {
    for (key, overlay_value) in overlay {
        match (base.get_mut(&key), overlay_value) {
            (Some(Value::Mapping(base_map)), Value::Mapping(overlay_map)) => {
                merge_mapping(base_map, overlay_map);
            }
            (_, overlay_value) => {
                base.insert(key, overlay_value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::DEFAULT_BIND_ADDRESS;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn returns_defaults_when_no_config_files_exist() {
        let config = load_config_from_paths(None, None).expect("config should load");
        assert_eq!(config.api.bind, DEFAULT_BIND_ADDRESS);
        assert!(config.security.prompt_injection.enabled);
        assert_eq!(config.models.providers.len(), 1);
    }

    #[test]
    fn project_config_overrides_user_config() {
        let dir = tempdir().expect("tempdir");
        let user_config = dir.path().join("user.yaml");
        let project_config = dir.path().join("project.yaml");

        fs::write(
            &user_config,
            "api:\n  bind: 127.0.0.1:9200\nsecurity:\n  prompt_injection:\n    enabled: false\n",
        )
        .expect("write user config");

        fs::write(
            &project_config,
            "api:\n  bind: 127.0.0.1:9300\nsecurity:\n  prompt_injection:\n    confidence_threshold: 0.9\n",
        )
        .expect("write project config");

        let config = load_config_from_paths(Some(&user_config), Some(&project_config))
            .expect("config should merge");

        assert_eq!(config.api.bind, "127.0.0.1:9300");
        assert!(!config.security.prompt_injection.enabled);
        assert!((config.security.prompt_injection.confidence_threshold - 0.9).abs() < f32::EPSILON);
        assert_eq!(config.models.providers.len(), 1);
    }

    #[test]
    fn validate_file_applies_defaults_for_missing_sections() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("config.yaml");
        fs::write(&config_path, "daemon:\n  log_level: debug\n").expect("write config");

        let config = validate_config_file(&config_path).expect("config should validate");
        assert_eq!(config.daemon.log_level, "debug");
        assert_eq!(config.api.bind, DEFAULT_BIND_ADDRESS);
        assert_eq!(config.models.providers.len(), 1);
    }

    #[test]
    fn mcp_stdio_requires_command() {
        let servers = vec![McpServerConfig {
            id: "local-files".to_string(),
            transport: hive_contracts::McpTransportConfig::Stdio,
            command: None,
            ..McpServerConfig::default()
        }];
        let error = validate_mcp_servers(&servers).expect_err("should be invalid");
        assert!(error.to_string().contains("must include a command"));
    }

    #[test]
    fn mcp_sse_requires_url() {
        let servers = vec![McpServerConfig {
            id: "sse-server".to_string(),
            transport: hive_contracts::McpTransportConfig::Sse,
            url: None,
            ..McpServerConfig::default()
        }];
        let error = validate_mcp_servers(&servers).expect_err("should be invalid");
        assert!(error.to_string().contains("must include a url"));
    }

    #[test]
    fn discover_paths_respects_hivemind_config_path_env() {
        let dir = tempdir().expect("tempdir");
        let config_file = dir.path().join("my-config.yaml");
        fs::write(&config_file, "").expect("write config");

        std::env::set_var("HIVEMIND_CONFIG_PATH", &config_file);
        let result = discover_paths();
        std::env::remove_var("HIVEMIND_CONFIG_PATH");

        let paths = result.expect("should resolve paths");
        assert_eq!(paths.config_path, config_file);
        assert_eq!(paths.hivemind_home, dir.path());
    }

    #[test]
    fn discover_paths_respects_hivemind_home_env() {
        let dir = tempdir().expect("tempdir");

        std::env::set_var("HIVEMIND_HOME", dir.path());
        std::env::remove_var("HIVEMIND_CONFIG_PATH");
        let result = discover_paths();
        std::env::remove_var("HIVEMIND_HOME");

        let paths = result.expect("should resolve paths");
        assert_eq!(paths.hivemind_home, dir.path());
        assert_eq!(paths.config_path, dir.path().join(DEFAULT_CONFIG_FILE));
    }

    #[test]
    fn discover_paths_uses_home_dir_by_default() {
        std::env::remove_var("HIVEMIND_CONFIG_PATH");
        std::env::remove_var("HIVEMIND_HOME");

        let paths = discover_paths().expect("should resolve paths");
        let expected = dirs::home_dir().map(|h| h.join(".hivemind"));
        assert_eq!(Some(paths.hivemind_home), expected, "hivemind_home should be ~/.hivemind");
    }

    #[test]
    fn archive_persona_sets_archived_flag() {
        let dir = tempdir().expect("tempdir");
        let persona = Persona {
            id: "user/test-bot".to_string(),
            name: "Test Bot".to_string(),
            archived: false,
            ..Persona::default_persona()
        };
        save_persona(dir.path(), &persona).expect("save");

        archive_persona(dir.path(), "user/test-bot").expect("archive");

        let loaded = load_personas(dir.path()).expect("load");
        let found = loaded.iter().find(|p| p.id == "user/test-bot").expect("should still exist");
        assert!(found.archived, "persona should be archived");
        assert_eq!(found.name, "Test Bot", "other fields preserved");
    }

    #[test]
    fn archive_persona_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let persona = Persona {
            id: "user/bot2".to_string(),
            name: "Bot 2".to_string(),
            archived: false,
            ..Persona::default_persona()
        };
        save_persona(dir.path(), &persona).expect("save");

        archive_persona(dir.path(), "user/bot2").expect("first archive");
        archive_persona(dir.path(), "user/bot2").expect("second archive should be ok");

        let loaded = load_personas(dir.path()).expect("load");
        let found = loaded.iter().find(|p| p.id == "user/bot2").expect("should exist");
        assert!(found.archived);
    }

    #[test]
    fn archive_persona_noop_for_missing_file() {
        let dir = tempdir().expect("tempdir");
        archive_persona(dir.path(), "user/nonexistent").expect("should succeed for missing file");
    }
}
