use std::io::{Seek, Write};

use crate::types::*;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Build an `.agentkit` ZIP archive from the given export request.
///
/// Writes the archive to `writer` (e.g. a `Vec<u8>` via `Cursor`, a `File`, etc.).
pub fn export_kit<W: Write + Seek>(
    request: &ExportRequest,
    writer: W,
) -> Result<(), AgentKitError> {
    let mut zip = ZipWriter::new(writer);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut manifest_personas = Vec::new();
    let mut manifest_workflows = Vec::new();

    // ── Personas ────────────────────────────────────────────────────
    for persona in &request.personas {
        let base_path = format!("personas/{}", &persona.id);

        // persona.yaml
        let yaml_path = format!("{}/persona.yaml", &base_path);
        zip.start_file(&yaml_path, options)?;
        zip.write_all(&persona.persona_yaml)?;

        // skill files
        for (rel_path, data) in &persona.skill_files {
            let full_path = format!("{}/{}", &base_path, rel_path);
            zip.start_file(&full_path, options)?;
            zip.write_all(data)?;
        }

        manifest_personas.push(ManifestPersonaEntry { id: persona.id.clone(), path: base_path });
    }

    // ── Workflows ───────────────────────────────────────────────────
    for workflow in &request.workflows {
        let base_path = format!("workflows/{}", &workflow.name);

        // workflow.yaml
        let yaml_path = format!("{}/workflow.yaml", &base_path);
        zip.start_file(&yaml_path, options)?;
        zip.write_all(&workflow.workflow_yaml)?;

        // attachment files
        for (filename, data) in &workflow.attachment_files {
            let full_path = format!("{}/attachments/{}", &base_path, filename);
            zip.start_file(&full_path, options)?;
            zip.write_all(data)?;
        }

        manifest_workflows
            .push(ManifestWorkflowEntry { name: workflow.name.clone(), path: base_path });
    }

    // ── Manifest ────────────────────────────────────────────────────
    let manifest = AgentKitManifest {
        format_version: FORMAT_VERSION,
        name: request.kit_name.clone(),
        description: request.description.clone(),
        author: request.author.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        hivemind_version: None,
        personas: manifest_personas,
        workflows: manifest_workflows,
    };

    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    zip.start_file(MANIFEST_PATH, options)?;
    zip.write_all(&manifest_json)?;

    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn export_empty_kit() {
        let request = ExportRequest {
            kit_name: "empty".to_string(),
            description: None,
            author: None,
            personas: vec![],
            workflows: vec![],
        };
        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();

        let bytes = buf.into_inner();
        assert!(!bytes.is_empty());

        // Verify it's a valid ZIP with a manifest
        let reader = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let manifest_file = archive.by_name(MANIFEST_PATH).unwrap();
        let manifest: AgentKitManifest = serde_json::from_reader(manifest_file).unwrap();
        assert_eq!(manifest.format_version, FORMAT_VERSION);
        assert_eq!(manifest.name, "empty");
        assert!(manifest.personas.is_empty());
        assert!(manifest.workflows.is_empty());
    }

    #[test]
    fn export_with_persona_and_workflow() {
        let request = ExportRequest {
            kit_name: "test-kit".to_string(),
            description: Some("A test kit".to_string()),
            author: Some("tester".to_string()),
            personas: vec![PersonaExportData {
                id: "acme/bot".to_string(),
                persona_yaml: b"id: acme/bot\nname: Bot\n".to_vec(),
                skill_files: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(
                        "skills/web-search/SKILL.md".to_string(),
                        b"---\nname: web-search\n---\nSearch the web.".to_vec(),
                    );
                    m
                },
            }],
            workflows: vec![WorkflowExportData {
                name: "acme/flow".to_string(),
                workflow_yaml: b"name: acme/flow\nsteps: []\n".to_vec(),
                attachment_files: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("abc_readme.pdf".to_string(), b"PDF-CONTENT".to_vec());
                    m
                },
            }],
        };

        let mut buf = Cursor::new(Vec::new());
        export_kit(&request, &mut buf).unwrap();
        let bytes = buf.into_inner();

        let reader = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();

        // Check manifest
        let manifest: AgentKitManifest = {
            let f = archive.by_name(MANIFEST_PATH).unwrap();
            serde_json::from_reader(f).unwrap()
        };
        assert_eq!(manifest.personas.len(), 1);
        assert_eq!(manifest.personas[0].id, "acme/bot");
        assert_eq!(manifest.workflows.len(), 1);
        assert_eq!(manifest.workflows[0].name, "acme/flow");

        // Check files exist
        assert!(archive.by_name("personas/acme/bot/persona.yaml").is_ok());
        assert!(archive.by_name("personas/acme/bot/skills/web-search/SKILL.md").is_ok());
        assert!(archive.by_name("workflows/acme/flow/workflow.yaml").is_ok());
        assert!(archive.by_name("workflows/acme/flow/attachments/abc_readme.pdf").is_ok());
    }
}
