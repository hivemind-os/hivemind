//! Bundled (factory-shipped) persona and workflow definitions.
//!
//! Persona YAML files are embedded at compile time via `include_str!()`.
//! Skill directories are embedded via `include_dir!()` so that bundled skills
//! can include the full directory tree (scripts/, references/, assets/, etc.)
//! as defined by the [Agent Skills specification](https://agentskills.io/specification).
//!
//! # Adding a new bundled persona
//! 1. Drop a `.yaml` file into `crates/hive-core/bundled-personas/`
//! 2. Add an `include_str!` entry to [`BUNDLED_PERSONA_YAMLS`] below
//!
//! # Adding bundled skills for a persona
//! 1. Create `bundled-personas/{namespace}/{name}/skills/{skill-name}/SKILL.md`
//!    (plus any scripts/, references/, assets/ subdirectories)
//! 2. Add an `include_dir!` static for the persona's `skills/` directory
//! 3. Add a match arm in [`bundled_skill_dir()`]
//!
//! # Adding a new bundled workflow
//! 1. Drop a `.yaml` file into `crates/hive-core/bundled-workflows/`
//! 2. Add an `include_str!` entry to [`BUNDLED_WORKFLOW_YAMLS`] below

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hive_contracts::Persona;

// ---------------------------------------------------------------------------
// Embedded YAML sources
// ---------------------------------------------------------------------------

/// (persona_id, yaml_content) pairs for all bundled personas.
static BUNDLED_PERSONA_YAMLS: &[(&str, &str)] = &[
    ("system/general", include_str!("../bundled-personas/general.yaml")),
    ("system/software/planner", include_str!("../bundled-personas/feature-planner.yaml")),
    ("system/software/implementor", include_str!("../bundled-personas/feature-implementor.yaml")),
    ("system/software/tester", include_str!("../bundled-personas/feature-tester.yaml")),
    ("system/software/reviewer", include_str!("../bundled-personas/feature-reviewer.yaml")),
    ("system/software/researcher", include_str!("../bundled-personas/feature-researcher.yaml")),
    ("system/software/spec-writer", include_str!("../bundled-personas/feature-spec-writer.yaml")),
    ("system/software/documenter", include_str!("../bundled-personas/feature-documenter.yaml")),
    ("system/3d-print/cad-designer", include_str!("../bundled-personas/3dprint-cad-designer.yaml")),
    ("system/3d-print/mesh-analyst", include_str!("../bundled-personas/3dprint-mesh-analyst.yaml")),
    (
        "system/3d-print/print-advisor",
        include_str!("../bundled-personas/3dprint-print-advisor.yaml"),
    ),
    (
        "system/finance/tax-advisor",
        include_str!("../bundled-personas/finance-tax-advisor.yaml"),
    ),
];

// ---------------------------------------------------------------------------
// Embedded skill directories (full directory trees, not just SKILL.md)
// ---------------------------------------------------------------------------

/// Embedded skills directory for the `system/general` persona.
/// Contains full skill directory trees (SKILL.md + scripts/ + references/ + assets/).
static BUNDLED_SKILLS_SYSTEM_GENERAL: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/bundled-personas/system/general/skills");

/// Embedded skills directory for the `system/3d-print/cad-designer` persona.
static BUNDLED_SKILLS_3DPRINT_CAD_DESIGNER: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/bundled-personas/system/3d-print/cad-designer/skills");

/// Embedded skills directory for the `system/finance/tax-advisor` persona.
static BUNDLED_SKILLS_FINANCE_TAX_ADVISOR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/bundled-personas/system/finance/tax-advisor/skills");

/// Return the embedded skills [`Dir`] for a bundled persona, if any.
///
/// Each top-level subdirectory within the returned `Dir` is a complete skill
/// package conforming to the [Agent Skills specification](https://agentskills.io/specification).
fn bundled_skill_dir(persona_id: &str) -> Option<&'static Dir<'static>> {
    match persona_id {
        "system/general" => Some(&BUNDLED_SKILLS_SYSTEM_GENERAL),
        "system/3d-print/cad-designer" => Some(&BUNDLED_SKILLS_3DPRINT_CAD_DESIGNER),
        "system/finance/tax-advisor" => Some(&BUNDLED_SKILLS_FINANCE_TAX_ADVISOR),
        // To add skills for more bundled personas, add match arms here.
        _ => None,
    }
}

/// (workflow_name, yaml_content) pairs for all bundled workflows.
static BUNDLED_WORKFLOW_YAMLS: &[(&str, &str)] = &[
    ("system/email-triage", include_str!("../bundled-workflows/email-triage.yaml")),
    ("system/email-responder", include_str!("../bundled-workflows/email-responder.yaml")),
    ("system/approval-workflow", include_str!("../bundled-workflows/approval-workflow.yaml")),
    ("system/software/major-feature", include_str!("../bundled-workflows/software-feature.yaml")),
    (
        "system/software/plan-and-implement",
        include_str!("../bundled-workflows/plan-and-implement.yaml"),
    ),
    ("system/3d-print/design", include_str!("../bundled-workflows/3dprint-design.yaml")),
];

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

/// Return the full list of bundled persona (id, yaml) pairs.
pub fn bundled_persona_yamls() -> &'static [(&'static str, &'static str)] {
    BUNDLED_PERSONA_YAMLS
}

/// Look up the factory YAML for a bundled persona by its ID.
pub fn bundled_persona_yaml(id: &str) -> Option<&'static str> {
    BUNDLED_PERSONA_YAMLS.iter().find(|(pid, _)| *pid == id).map(|(_, yaml)| *yaml)
}

/// Return the full list of bundled workflow (name, yaml) pairs.
pub fn bundled_workflow_yamls() -> &'static [(&'static str, &'static str)] {
    BUNDLED_WORKFLOW_YAMLS
}

/// Look up the factory YAML for a bundled workflow by its name.
pub fn bundled_workflow_yaml(name: &str) -> Option<&'static str> {
    BUNDLED_WORKFLOW_YAMLS.iter().find(|(n, _)| *n == name).map(|(_, yaml)| *yaml)
}

/// Check whether a persona ID corresponds to a bundled persona.
pub fn is_bundled_persona(id: &str) -> bool {
    BUNDLED_PERSONA_YAMLS.iter().any(|(pid, _)| *pid == id)
}

/// Check whether a workflow name corresponds to a bundled workflow.
pub fn is_bundled_workflow(name: &str) -> bool {
    BUNDLED_WORKFLOW_YAMLS.iter().any(|(n, _)| *n == name)
}

/// Return the bundled skill names for a given persona ID.
pub fn bundled_persona_skill_names(persona_id: &str) -> Vec<&'static str> {
    match bundled_skill_dir(persona_id) {
        Some(dir) => {
            dir.dirs().filter_map(|d| d.path().file_name().and_then(|n| n.to_str())).collect()
        }
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Content hashing
// ---------------------------------------------------------------------------

fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn sha256_hex(data: &str) -> String {
    sha256_bytes(data.as_bytes())
}

// ---------------------------------------------------------------------------
// Persona seeding
// ---------------------------------------------------------------------------

/// Path to the checksums file within the personas directory.
fn checksums_path(personas_dir: &Path) -> std::path::PathBuf {
    personas_dir.join(".bundled-checksums.json")
}

fn load_checksums(personas_dir: &Path) -> HashMap<String, String> {
    let path = checksums_path(personas_dir);
    if !path.exists() {
        return HashMap::new();
    }
    let data = fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_checksums(personas_dir: &Path, checksums: &HashMap<String, String>) -> Result<()> {
    let path = checksums_path(personas_dir);
    let data = serde_json::to_string_pretty(checksums)?;
    fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))
}

/// Seed bundled personas into `personas_dir`.
///
/// - If a persona file does not exist, write the factory YAML.
/// - If a persona file exists and its content matches the stored factory hash
///   (i.e. the user has not modified it), overwrite with the (possibly newer)
///   factory YAML from the current binary.
/// - If the content has been modified by the user, leave it alone.
///
/// Returns the number of personas written (first-run + auto-updated).
pub fn seed_bundled_personas(personas_dir: &Path) -> Result<usize> {
    fs::create_dir_all(personas_dir).with_context(|| {
        format!("failed to create personas directory {}", personas_dir.display())
    })?;

    let mut checksums = load_checksums(personas_dir);
    let mut written = 0;

    for &(id, factory_yaml) in BUNDLED_PERSONA_YAMLS {
        let persona_path = id.replace('/', std::path::MAIN_SEPARATOR_STR);
        let persona_dir = personas_dir.join(&persona_path);
        fs::create_dir_all(&persona_dir).with_context(|| {
            format!("failed to create persona directory {}", persona_dir.display())
        })?;
        let file_path = persona_dir.join("persona.yaml");
        let factory_hash = sha256_hex(factory_yaml);

        if file_path.exists() {
            // Check whether the user has modified the file.
            let on_disk = fs::read_to_string(&file_path).unwrap_or_default();
            let on_disk_hash = sha256_hex(&on_disk);

            let stored_factory_hash = checksums.get(id).cloned().unwrap_or_default();

            if on_disk_hash == stored_factory_hash || on_disk_hash == factory_hash {
                // User has NOT modified (matches old or new factory content).
                if on_disk_hash != factory_hash {
                    // New factory version — auto-update, preserving archived state.
                    let existing: Persona = serde_yaml::from_str(&on_disk)
                        .ok()
                        .unwrap_or_else(Persona::default_persona);
                    let mut updated: Persona = serde_yaml::from_str(factory_yaml)
                        .context("failed to parse bundled persona YAML")?;
                    updated.archived = existing.archived;
                    updated.bundled = true;
                    let yaml_out = serde_yaml::to_string(&updated)?;
                    fs::write(&file_path, &yaml_out)?;
                    checksums.insert(id.to_string(), sha256_hex(&yaml_out));
                    written += 1;
                    tracing::info!(
                        persona_id = id,
                        "auto-updated bundled persona to new factory version"
                    );
                } else {
                    // Already up-to-date; just make sure checksum is recorded.
                    checksums.insert(id.to_string(), factory_hash);
                }
            }
            // else: user has modified — leave alone, keep existing checksum
        } else {
            // First run — write factory YAML.
            let mut persona: Persona = serde_yaml::from_str(factory_yaml)
                .context("failed to parse bundled persona YAML")?;
            persona.bundled = true;
            let yaml_out = serde_yaml::to_string(&persona)?;
            fs::write(&file_path, &yaml_out)?;
            checksums.insert(id.to_string(), sha256_hex(&yaml_out));
            written += 1;
            tracing::info!(persona_id = id, "seeded bundled persona");
        }

        // Seed bundled skills for this persona.
        seed_bundled_skills(personas_dir, id, &mut checksums, false)?;
    }

    save_checksums(personas_dir, &checksums)?;
    Ok(written)
}

/// Reset a bundled persona to its factory YAML.
///
/// Overwrites the file and updates the checksum.  Also re-seeds bundled skills.
/// Returns the freshly written [`Persona`] or `None` if the given ID is not a
/// bundled persona.
pub fn reset_bundled_persona(personas_dir: &Path, id: &str) -> Result<Option<Persona>> {
    let factory_yaml = match bundled_persona_yaml(id) {
        Some(y) => y,
        None => return Ok(None),
    };
    let persona_path = id.replace('/', std::path::MAIN_SEPARATOR_STR);
    let persona_dir = personas_dir.join(&persona_path);
    fs::create_dir_all(&persona_dir)?;
    let file_path = persona_dir.join("persona.yaml");

    let mut persona: Persona =
        serde_yaml::from_str(factory_yaml).context("failed to parse bundled persona YAML")?;
    persona.bundled = true;
    // Resetting un-archives the persona.
    persona.archived = false;
    let yaml_out = serde_yaml::to_string(&persona)?;

    fs::write(&file_path, &yaml_out)?;

    let mut checksums = load_checksums(personas_dir);
    checksums.insert(id.to_string(), sha256_hex(&yaml_out));

    // Re-seed bundled skills (force overwrite to factory state).
    seed_bundled_skills(personas_dir, id, &mut checksums, true)?;

    save_checksums(personas_dir, &checksums)?;

    Ok(Some(persona))
}

/// Write bundled skills for a persona into its `skills/` subdirectory.
///
/// Each skill is a full directory tree (SKILL.md plus optional scripts/,
/// references/, assets/, etc.) conforming to the Agent Skills specification.
///
/// When `force` is false (normal seeding / upgrade):
/// - New files are written and their factory hash recorded.
/// - Existing files whose on-disk hash matches the stored factory hash (i.e. the
///   user has NOT modified them) are auto-updated to the new factory content.
/// - User-modified files are left alone.
///
/// When `force` is true (reset): all files are overwritten unconditionally.
fn seed_bundled_skills(
    personas_dir: &Path,
    persona_id: &str,
    checksums: &mut HashMap<String, String>,
    force: bool,
) -> Result<()> {
    let bundled_dir = match bundled_skill_dir(persona_id) {
        Some(d) => d,
        None => return Ok(()),
    };

    let persona_path = persona_id.replace('/', std::path::MAIN_SEPARATOR_STR);
    let skills_dir = personas_dir.join(&persona_path).join("skills");
    fs::create_dir_all(&skills_dir)
        .with_context(|| format!("failed to create skills directory {}", skills_dir.display()))?;

    // Each top-level subdirectory in the embedded Dir is one skill.
    for skill_entry in bundled_dir.dirs() {
        let skill_name =
            skill_entry.path().file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
        let target_skill_dir = skills_dir.join(skill_name);
        write_bundled_dir_recursive(
            skill_entry,
            skill_entry.path(),
            &target_skill_dir,
            persona_id,
            skill_name,
            checksums,
            force,
        )?;
        tracing::info!(persona_id, skill_name, "seeded bundled skill");
    }

    Ok(())
}

/// Recursively write all files from an embedded [`Dir`] to the target path,
/// tracking per-file checksums so that factory updates are applied automatically
/// while user-modified files are preserved.
fn write_bundled_dir_recursive(
    dir: &Dir<'_>,
    base: &Path,
    target: &Path,
    persona_id: &str,
    skill_name: &str,
    checksums: &mut HashMap<String, String>,
    force: bool,
) -> Result<()> {
    fs::create_dir_all(target)?;

    for file in dir.files() {
        let rel = file.path().strip_prefix(base).unwrap_or(file.path());
        let dest = target.join(rel);
        let checksum_key = format!("skill:{}:{}:{}", persona_id, skill_name, rel.display());
        let factory_hash = sha256_bytes(file.contents());

        if force {
            // Reset mode — always overwrite.
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, file.contents())?;
            checksums.insert(checksum_key, factory_hash);
        } else if dest.exists() {
            let on_disk = fs::read(&dest).unwrap_or_default();
            let on_disk_hash = sha256_bytes(&on_disk);
            let stored_hash = checksums.get(&checksum_key).cloned().unwrap_or_default();

            if on_disk_hash == stored_hash || on_disk_hash == factory_hash {
                // User has NOT modified — auto-update if factory changed.
                if on_disk_hash != factory_hash {
                    fs::write(&dest, file.contents())?;
                }
                checksums.insert(checksum_key, factory_hash);
            }
            // else: user modified — leave alone
        } else {
            // New file — write it.
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, file.contents())?;
            checksums.insert(checksum_key, factory_hash);
        }
    }

    for sub in dir.dirs() {
        let rel = sub.path().strip_prefix(base).unwrap_or(sub.path());
        let sub_target = target.join(rel);
        write_bundled_dir_recursive(
            sub,
            base,
            &sub_target,
            persona_id,
            skill_name,
            checksums,
            force,
        )?;
    }

    Ok(())
}
