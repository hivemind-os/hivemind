use std::collections::HashMap;
use std::io::{Read, Seek};

use crate::rewrite::{
    build_remap_table, collect_references, remap_id, rewrite_workflow_references,
};
use crate::types::*;

/// Preview what would happen when importing an `.agentkit` archive.
///
/// This function has no side effects — it reads the archive, validates the
/// target namespace, computes new IDs, and checks for conflicts.
pub fn preview_import<R: Read + Seek>(
    reader: R,
    target_namespace: &str,
    persona_checker: &dyn PersonaSaver,
    workflow_checker: &dyn WorkflowSaver,
) -> Result<ImportPreview, AgentKitError> {
    // Validate target namespace
    let mut errors = Vec::new();
    if target_namespace == "system" || target_namespace.starts_with("system/") {
        errors.push("Cannot import into the 'system/' namespace".to_string());
    }
    if let Err(e) = hive_contracts::validate_namespaced_id(
        &format!("{target_namespace}/placeholder"),
        "Target namespace",
    ) {
        errors.push(e);
    }

    // Parse the archive
    let mut archive = zip::ZipArchive::new(reader)?;

    // Read manifest
    let manifest = read_manifest(&mut archive)?;

    if manifest.format_version != FORMAT_VERSION {
        return Err(AgentKitError::UnsupportedVersion(manifest.format_version));
    }

    // Build preview items
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    // Build remap table for reference analysis
    let persona_ids: Vec<String> = manifest.personas.iter().map(|p| p.id.clone()).collect();
    let workflow_names: Vec<String> = manifest.workflows.iter().map(|w| w.name.clone()).collect();
    let remap = build_remap_table(&persona_ids, &workflow_names, target_namespace);

    for persona_entry in &manifest.personas {
        let new_id = remap_id(&persona_entry.id, target_namespace);
        let exists = if errors.is_empty() {
            persona_checker.persona_exists(&new_id).unwrap_or(false)
        } else {
            false
        };
        items.push(ImportPreviewItem {
            kind: ImportItemKind::Persona,
            original_id: persona_entry.id.clone(),
            new_id,
            overwrites_existing: exists,
        });
    }

    for wf_entry in &manifest.workflows {
        let new_name = remap_id(&wf_entry.name, target_namespace);
        let exists = if errors.is_empty() {
            workflow_checker.workflow_exists(&new_name).unwrap_or(false)
        } else {
            false
        };
        items.push(ImportPreviewItem {
            kind: ImportItemKind::Workflow,
            original_id: wf_entry.name.clone(),
            new_id: new_name,
            overwrites_existing: exists,
        });

        // Check for external references
        let yaml_path = format!("{}/workflow.yaml", &wf_entry.path);
        if let Ok(yaml_str) = read_archive_string(&mut archive, &yaml_path) {
            if let Ok(refs) = collect_references(&yaml_str) {
                for r in refs {
                    if !remap.contains_key(&r) {
                        warnings.push(format!(
                            "Workflow '{}' references '{}' which is not included in this kit",
                            wf_entry.name, r
                        ));
                    }
                }
            }
        }
    }

    Ok(ImportPreview {
        manifest,
        target_namespace: target_namespace.to_string(),
        items,
        errors,
        warnings,
    })
}

/// Apply an import from an `.agentkit` archive.
///
/// Only items whose `new_id` appears in `request.selected_items` are imported.
/// Cross-references are rewritten according to the remap table.
pub fn apply_import<R: Read + Seek>(
    reader: R,
    request: &ImportApplyRequest,
    persona_saver: &dyn PersonaSaver,
    workflow_saver: &dyn WorkflowSaver,
) -> Result<ImportResult, AgentKitError> {
    // Validate namespace
    if request.target_namespace == "system" || request.target_namespace.starts_with("system/") {
        return Err(AgentKitError::InvalidNamespace(
            "Cannot import into the 'system/' namespace".to_string(),
        ));
    }

    let mut archive = zip::ZipArchive::new(reader)?;
    let manifest = read_manifest(&mut archive)?;

    if manifest.format_version != FORMAT_VERSION {
        return Err(AgentKitError::UnsupportedVersion(manifest.format_version));
    }

    // Build remap table
    let persona_ids: Vec<String> = manifest.personas.iter().map(|p| p.id.clone()).collect();
    let workflow_names: Vec<String> = manifest.workflows.iter().map(|w| w.name.clone()).collect();
    let remap = build_remap_table(&persona_ids, &workflow_names, &request.target_namespace);

    let selected: std::collections::HashSet<&str> =
        request.selected_items.iter().map(|s| s.as_str()).collect();

    let mut result = ImportResult {
        imported_personas: Vec::new(),
        imported_workflows: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
    };

    // ── Import personas ─────────────────────────────────────────────
    for entry in &manifest.personas {
        let new_id = remap_id(&entry.id, &request.target_namespace);
        if !selected.contains(new_id.as_str()) {
            result.skipped.push(new_id);
            continue;
        }

        match import_persona(&mut archive, entry, &new_id, persona_saver) {
            Ok(overwritten) => {
                result.imported_personas.push(ImportedItem {
                    original_id: entry.id.clone(),
                    new_id,
                    overwritten,
                });
            }
            Err(e) => {
                result.errors.push(ImportError { item_id: new_id, message: e.to_string() });
            }
        }
    }

    // ── Import workflows ────────────────────────────────────────────
    for entry in &manifest.workflows {
        let new_name = remap_id(&entry.name, &request.target_namespace);
        if !selected.contains(new_name.as_str()) {
            result.skipped.push(new_name);
            continue;
        }

        match import_workflow(&mut archive, entry, &new_name, &remap, workflow_saver) {
            Ok(overwritten) => {
                result.imported_workflows.push(ImportedItem {
                    original_id: entry.name.clone(),
                    new_id: new_name,
                    overwritten,
                });
            }
            Err(e) => {
                result.errors.push(ImportError { item_id: new_name, message: e.to_string() });
            }
        }
    }

    Ok(result)
}

// ── Helpers ─────────────────────────────────────────────────────────

fn read_manifest<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<AgentKitManifest, AgentKitError> {
    let file = archive.by_name(MANIFEST_PATH).map_err(|_| {
        AgentKitError::InvalidArchive("missing manifest.json in archive".to_string())
    })?;
    let manifest: AgentKitManifest = serde_json::from_reader(file)?;
    Ok(manifest)
}

fn read_archive_bytes<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Vec<u8>, AgentKitError> {
    let mut file = archive
        .by_name(path)
        .map_err(|_| AgentKitError::InvalidArchive(format!("missing file in archive: {path}")))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_archive_string<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<String, AgentKitError> {
    let bytes = read_archive_bytes(archive, path)?;
    String::from_utf8(bytes).map_err(|e| AgentKitError::InvalidArchive(e.to_string()))
}

fn import_persona<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry: &ManifestPersonaEntry,
    new_id: &str,
    saver: &dyn PersonaSaver,
) -> Result<bool, AgentKitError> {
    let overwritten =
        saver.persona_exists(new_id).map_err(|e| AgentKitError::Other(e.to_string()))?;

    // Read persona.yaml and rewrite the ID
    let yaml_path = format!("{}/persona.yaml", &entry.path);
    let yaml_bytes = read_archive_bytes(archive, &yaml_path)?;
    let persona_yaml = rewrite_persona_id(&yaml_bytes, new_id)?;

    // Collect skill files
    let skills_prefix = format!("{}/skills/", &entry.path);
    let skill_file_paths: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let file = archive.by_index(i).ok()?;
            let name = file.name().to_string();
            if name.starts_with(&skills_prefix) && !file.is_dir() {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut skill_files = HashMap::new();
    for full_path in skill_file_paths {
        let rel_path =
            full_path.strip_prefix(&format!("{}/", &entry.path)).unwrap_or(&full_path).to_string();
        let data = read_archive_bytes(archive, &full_path)?;
        skill_files.insert(rel_path, data);
    }

    saver
        .save_persona(new_id, &persona_yaml, &skill_files)
        .map_err(|e| AgentKitError::Other(e.to_string()))?;

    Ok(overwritten)
}

fn import_workflow<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry: &ManifestWorkflowEntry,
    new_name: &str,
    remap: &HashMap<String, String>,
    saver: &dyn WorkflowSaver,
) -> Result<bool, AgentKitError> {
    let overwritten =
        saver.workflow_exists(new_name).map_err(|e| AgentKitError::Other(e.to_string()))?;

    // Read workflow YAML and rewrite references
    let yaml_path = format!("{}/workflow.yaml", &entry.path);
    let yaml_str = read_archive_string(archive, &yaml_path)?;
    let (rewritten_yaml, _) = rewrite_workflow_references(&yaml_str, remap)?;

    // Also rewrite the workflow name itself
    let final_yaml = rewrite_workflow_name(&rewritten_yaml, new_name)?;

    // Generate a new unique ID so the imported definition doesn't collide
    // with the original workflow's external_id in the store.
    let final_yaml = rewrite_workflow_id(&final_yaml)?;

    // Collect attachment files
    let attachments_prefix = format!("{}/attachments/", &entry.path);
    let attachment_paths: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let file = archive.by_index(i).ok()?;
            let name = file.name().to_string();
            if name.starts_with(&attachments_prefix) && !file.is_dir() {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut attachment_files = HashMap::new();
    for full_path in attachment_paths {
        let filename =
            full_path.strip_prefix(&attachments_prefix).unwrap_or(&full_path).to_string();
        let data = read_archive_bytes(archive, &full_path)?;
        attachment_files.insert(filename, data);
    }

    saver
        .save_workflow(new_name, final_yaml.as_bytes(), &attachment_files)
        .map_err(|e| AgentKitError::Other(e.to_string()))?;

    Ok(overwritten)
}

/// Rewrite the `id` field in a persona YAML to the new ID.
fn rewrite_persona_id(yaml_bytes: &[u8], new_id: &str) -> Result<Vec<u8>, AgentKitError> {
    let yaml_str = std::str::from_utf8(yaml_bytes)
        .map_err(|e| AgentKitError::InvalidArchive(e.to_string()))?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(yaml_str)?;

    if let serde_yaml::Value::Mapping(ref mut map) = value {
        map.insert(
            serde_yaml::Value::String("id".to_string()),
            serde_yaml::Value::String(new_id.to_string()),
        );
    }

    let output = serde_yaml::to_string(&value)?;
    Ok(output.into_bytes())
}

/// Rewrite the `name` field in a workflow YAML to the new name.
fn rewrite_workflow_name(yaml: &str, new_name: &str) -> Result<String, AgentKitError> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(yaml)?;

    if let serde_yaml::Value::Mapping(ref mut map) = value {
        map.insert(
            serde_yaml::Value::String("name".to_string()),
            serde_yaml::Value::String(new_name.to_string()),
        );
    }

    Ok(serde_yaml::to_string(&value)?)
}

/// Replace the `id` field in a workflow YAML with a freshly generated UUID.
///
/// Imported workflows are new definitions and must have unique IDs so they
/// don't collide with the original workflow's `external_id` in the store.
fn rewrite_workflow_id(yaml: &str) -> Result<String, AgentKitError> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(yaml)?;

    if let serde_yaml::Value::Mapping(ref mut map) = value {
        map.insert(
            serde_yaml::Value::String("id".to_string()),
            serde_yaml::Value::String(uuid::Uuid::new_v4().to_string()),
        );
    }

    Ok(serde_yaml::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::export_kit;
    use std::io::{Cursor, Write};
    use std::sync::Mutex;

    /// In-memory saver for testing.
    struct MemPersonaSaver {
        existing: Vec<String>,
        saved: Mutex<Vec<(String, Vec<u8>, HashMap<String, Vec<u8>>)>>,
    }

    impl MemPersonaSaver {
        fn new(existing: Vec<String>) -> Self {
            Self { existing, saved: Mutex::new(Vec::new()) }
        }
    }

    impl PersonaSaver for MemPersonaSaver {
        fn save_persona(
            &self,
            new_id: &str,
            persona_yaml: &[u8],
            skill_files: &HashMap<String, Vec<u8>>,
        ) -> Result<(), anyhow::Error> {
            self.saved.lock().unwrap().push((
                new_id.to_string(),
                persona_yaml.to_vec(),
                skill_files.clone(),
            ));
            Ok(())
        }

        fn persona_exists(&self, id: &str) -> Result<bool, anyhow::Error> {
            Ok(self.existing.contains(&id.to_string()))
        }
    }

    struct MemWorkflowSaver {
        existing: Vec<String>,
        saved: Mutex<Vec<(String, Vec<u8>, HashMap<String, Vec<u8>>)>>,
    }

    impl MemWorkflowSaver {
        fn new(existing: Vec<String>) -> Self {
            Self { existing, saved: Mutex::new(Vec::new()) }
        }
    }

    impl WorkflowSaver for MemWorkflowSaver {
        fn save_workflow(
            &self,
            new_name: &str,
            workflow_yaml: &[u8],
            attachment_files: &HashMap<String, Vec<u8>>,
        ) -> Result<(), anyhow::Error> {
            self.saved.lock().unwrap().push((
                new_name.to_string(),
                workflow_yaml.to_vec(),
                attachment_files.clone(),
            ));
            Ok(())
        }

        fn workflow_exists(&self, name: &str) -> Result<bool, anyhow::Error> {
            Ok(self.existing.contains(&name.to_string()))
        }
    }

    fn make_test_kit() -> Vec<u8> {
        let request = ExportRequest {
            kit_name: "test".to_string(),
            description: None,
            author: None,
            personas: vec![PersonaExportData {
                id: "acme/bot".to_string(),
                persona_yaml: b"id: acme/bot\nname: Bot\nsystem_prompt: hello\n".to_vec(),
                skill_files: {
                    let mut m = HashMap::new();
                    m.insert(
                        "skills/search/SKILL.md".to_string(),
                        b"---\nname: search\n---\nSearch skill".to_vec(),
                    );
                    m
                },
            }],
            workflows: vec![WorkflowExportData {
                name: "acme/flow".to_string(),
                workflow_yaml: br#"name: acme/flow
steps:
  - id: t1
    type: trigger
    trigger:
      kind: manual
    next:
      - s1
  - id: s1
    type: task
    task:
      kind: invoke_agent
      persona_id: acme/bot
      task: do work
"#
                .to_vec(),
                attachment_files: {
                    let mut m = HashMap::new();
                    m.insert("doc.pdf".to_string(), b"PDFDATA".to_vec());
                    m
                },
            }],
        };
        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();
        buf.into_inner()
    }

    #[test]
    fn preview_detects_no_conflicts() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let preview = preview_import(Cursor::new(&kit), "myteam", &ps, &ws).unwrap();

        assert!(preview.errors.is_empty());
        assert_eq!(preview.items.len(), 2);
        assert_eq!(preview.items[0].new_id, "myteam/bot");
        assert!(!preview.items[0].overwrites_existing);
        assert_eq!(preview.items[1].new_id, "myteam/flow");
        assert!(!preview.items[1].overwrites_existing);
    }

    #[test]
    fn preview_detects_conflicts() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec!["myteam/bot".to_string()]);
        let ws = MemWorkflowSaver::new(vec!["myteam/flow".to_string()]);

        let preview = preview_import(Cursor::new(&kit), "myteam", &ps, &ws).unwrap();

        assert!(preview.items[0].overwrites_existing);
        assert!(preview.items[1].overwrites_existing);
    }

    #[test]
    fn preview_rejects_system_namespace() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let preview = preview_import(Cursor::new(&kit), "system", &ps, &ws).unwrap();
        assert!(!preview.errors.is_empty());
        assert!(preview.errors[0].contains("system"));
    }

    #[test]
    fn preview_warns_about_external_refs() {
        // Create a kit where workflow references a persona NOT in the kit
        let request = ExportRequest {
            kit_name: "test".to_string(),
            description: None,
            author: None,
            personas: vec![],
            workflows: vec![WorkflowExportData {
                name: "acme/flow".to_string(),
                workflow_yaml: br#"name: acme/flow
steps:
  - id: s1
    type: task
    task:
      kind: invoke_agent
      persona_id: external/agent
      task: do work
"#
                .to_vec(),
                attachment_files: HashMap::new(),
            }],
        };
        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();
        let kit = buf.into_inner();

        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let preview = preview_import(Cursor::new(&kit), "myteam", &ps, &ws).unwrap();
        assert!(!preview.warnings.is_empty());
        assert!(preview.warnings[0].contains("external/agent"));
    }

    #[test]
    fn apply_import_round_trip() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let request = ImportApplyRequest {
            target_namespace: "myteam".to_string(),
            selected_items: vec!["myteam/bot".to_string(), "myteam/flow".to_string()],
        };

        let result = apply_import(Cursor::new(&kit), &request, &ps, &ws).unwrap();

        assert_eq!(result.imported_personas.len(), 1);
        assert_eq!(result.imported_personas[0].new_id, "myteam/bot");
        assert!(!result.imported_personas[0].overwritten);

        assert_eq!(result.imported_workflows.len(), 1);
        assert_eq!(result.imported_workflows[0].new_id, "myteam/flow");

        // Verify persona was saved with new ID
        let saved_personas = ps.saved.lock().unwrap();
        assert_eq!(saved_personas.len(), 1);
        let (id, yaml, skills) = &saved_personas[0];
        assert_eq!(id, "myteam/bot");
        let yaml_str = std::str::from_utf8(yaml).unwrap();
        assert!(yaml_str.contains("myteam/bot"));
        assert!(skills.contains_key("skills/search/SKILL.md"));

        // Verify workflow was saved with rewritten refs
        let saved_workflows = ws.saved.lock().unwrap();
        assert_eq!(saved_workflows.len(), 1);
        let (name, yaml, attachments) = &saved_workflows[0];
        assert_eq!(name, "myteam/flow");
        let yaml_str = std::str::from_utf8(yaml).unwrap();
        assert!(yaml_str.contains("persona_id: myteam/bot"));
        assert!(yaml_str.contains("name: myteam/flow"));
        assert!(attachments.contains_key("doc.pdf"));
    }

    #[test]
    fn apply_import_selective() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        // Only select the workflow, not the persona
        let request = ImportApplyRequest {
            target_namespace: "myteam".to_string(),
            selected_items: vec!["myteam/flow".to_string()],
        };

        let result = apply_import(Cursor::new(&kit), &request, &ps, &ws).unwrap();
        assert!(result.imported_personas.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.imported_workflows.len(), 1);
    }

    #[test]
    fn apply_import_rejects_system_namespace() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let request = ImportApplyRequest {
            target_namespace: "system".to_string(),
            selected_items: vec!["system/bot".to_string()],
        };

        let result = apply_import(Cursor::new(&kit), &request, &ps, &ws);
        assert!(result.is_err());
    }

    #[test]
    fn apply_import_detects_overwrite() {
        let kit = make_test_kit();
        let ps = MemPersonaSaver::new(vec!["myteam/bot".to_string()]);
        let ws = MemWorkflowSaver::new(vec![]);

        let request = ImportApplyRequest {
            target_namespace: "myteam".to_string(),
            selected_items: vec!["myteam/bot".to_string(), "myteam/flow".to_string()],
        };

        let result = apply_import(Cursor::new(&kit), &request, &ps, &ws).unwrap();
        assert!(result.imported_personas[0].overwritten);
        assert!(!result.imported_workflows[0].overwritten);
    }

    #[test]
    fn invalid_archive_missing_manifest() {
        // Create a valid ZIP but without manifest.json
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file("random.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"hello").unwrap();
            zip.finish().unwrap();
        }

        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);
        let result = preview_import(Cursor::new(buf.into_inner()), "myteam", &ps, &ws);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_archive_not_zip() {
        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);
        let result = preview_import(Cursor::new(b"not a zip file".to_vec()), "myteam", &ps, &ws);
        assert!(result.is_err());
    }

    #[test]
    fn multi_level_namespace_remap() {
        // Persona "a/b/bot" should become "x/y/b/bot" when target is "x/y"
        let request = ExportRequest {
            kit_name: "test".to_string(),
            description: None,
            author: None,
            personas: vec![PersonaExportData {
                id: "a/b/bot".to_string(),
                persona_yaml: b"id: a/b/bot\nname: Bot\n".to_vec(),
                skill_files: HashMap::new(),
            }],
            workflows: vec![],
        };
        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();
        let kit = buf.into_inner();

        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);
        let preview = preview_import(Cursor::new(&kit), "x/y", &ps, &ws).unwrap();

        // "a/b/bot" -> root "a" replaced with "x/y" -> "x/y/b/bot"
        assert_eq!(preview.items[0].new_id, "x/y/b/bot");
    }

    #[test]
    fn import_generates_new_workflow_id() {
        // Workflow YAML with an explicit `id` field (simulating an exported workflow)
        let original_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let workflow_yaml = format!(
            "id: {}\nname: acme/flow\nsteps:\n  - id: t1\n    type: trigger\n    trigger:\n      kind: manual\n    next:\n      - s1\n  - id: s1\n    type: task\n    task:\n      kind: invoke_agent\n      persona_id: acme/bot\n      task: do work\n",
            original_id
        );

        let request = ExportRequest {
            kit_name: "test".to_string(),
            description: None,
            author: None,
            personas: vec![],
            workflows: vec![WorkflowExportData {
                name: "acme/flow".to_string(),
                workflow_yaml: workflow_yaml.into_bytes(),
                attachment_files: HashMap::new(),
            }],
        };
        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();
        let kit = buf.into_inner();

        let ps = MemPersonaSaver::new(vec![]);
        let ws = MemWorkflowSaver::new(vec![]);

        let apply_request = ImportApplyRequest {
            target_namespace: "newns".to_string(),
            selected_items: vec!["newns/flow".to_string()],
        };

        let result = apply_import(Cursor::new(&kit), &apply_request, &ps, &ws).unwrap();
        assert_eq!(result.imported_workflows.len(), 1);

        // Verify the saved YAML has a new id, not the original
        let saved = ws.saved.lock().unwrap();
        let (_, yaml_bytes, _) = &saved[0];
        let yaml_str = std::str::from_utf8(yaml_bytes).unwrap();
        assert!(
            !yaml_str.contains(original_id),
            "imported workflow should have a new id, but still contains the original: {}",
            original_id
        );
        // Verify the YAML has an `id:` field (a new UUID was written)
        assert!(yaml_str.contains("id:"), "imported workflow YAML should have an id field");
    }
}
