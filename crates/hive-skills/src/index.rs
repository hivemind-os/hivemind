//! Local SQLite index for discovered and installed skills.

use hive_contracts::{
    DiscoveredSkill, InstalledSkill, SkillAuditResult, SkillManifest, SkillSourceConfig,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::Path;

/// Trait abstracting the skill index storage operations.
pub trait SkillIndexStore: Send + Sync {
    fn save_source(&self, config: &SkillSourceConfig) -> Result<(), IndexError>;
    fn list_sources(&self) -> Result<Vec<SkillSourceConfig>, IndexError>;
    fn clear_discovered(&self) -> Result<(), IndexError>;
    fn insert_discovered(&self, skills: &[DiscoveredSkill]) -> Result<(), IndexError>;
    fn list_discovered(&self, persona_id: Option<&str>)
        -> Result<Vec<DiscoveredSkill>, IndexError>;
    fn install_skill(&self, skill: &InstalledSkill) -> Result<(), IndexError>;
    fn uninstall_skill(&self, name: &str, persona_id: Option<&str>) -> Result<bool, IndexError>;
    fn set_skill_enabled(
        &self,
        name: &str,
        persona_id: Option<&str>,
        enabled: bool,
    ) -> Result<bool, IndexError>;
    fn list_installed(&self, persona_id: Option<&str>) -> Result<Vec<InstalledSkill>, IndexError>;
    fn get_installed(
        &self,
        name: &str,
        persona_id: Option<&str>,
    ) -> Result<Option<InstalledSkill>, IndexError>;
    fn list_enabled(&self, persona_id: Option<&str>) -> Result<Vec<InstalledSkill>, IndexError>;
}

/// Local skill index backed by SQLite.
pub struct SqliteSkillIndex {
    conn: Mutex<Connection>,
}

/// Backward-compatible type alias.
pub type SkillIndex = SqliteSkillIndex;

impl SqliteSkillIndex {
    pub fn open(db_path: &Path) -> Result<Self, IndexError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| IndexError::Io(e.to_string()))?;
        }
        let conn = Connection::open(db_path).map_err(|e| IndexError::Database(e.to_string()))?;
        let index = Self { conn: Mutex::new(conn) };
        index.migrate()?;
        Ok(index)
    }

    /// Create an in-memory index (for testing).
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, IndexError> {
        let conn = Connection::open_in_memory().map_err(|e| IndexError::Database(e.to_string()))?;
        let index = Self { conn: Mutex::new(conn) };
        index.migrate()?;
        Ok(index)
    }

    fn migrate(&self) -> Result<(), IndexError> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS skill_sources (
                source_id   TEXT PRIMARY KEY,
                config_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS discovered_skills (
                name        TEXT NOT NULL,
                source_id   TEXT NOT NULL,
                source_path TEXT NOT NULL,
                description TEXT NOT NULL,
                manifest_json TEXT NOT NULL,
                PRIMARY KEY (name, source_id)
            );

            CREATE TABLE IF NOT EXISTS installed_skills (
                name          TEXT PRIMARY KEY,
                source_id     TEXT NOT NULL,
                source_path   TEXT NOT NULL,
                local_path    TEXT NOT NULL,
                manifest_json TEXT NOT NULL,
                audit_json    TEXT NOT NULL,
                enabled       INTEGER NOT NULL DEFAULT 1,
                installed_at_ms INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| IndexError::Database(e.to_string()))?;

        // Migration: add persona_id column if it doesn't exist
        let _ = conn.execute(
            "ALTER TABLE installed_skills ADD COLUMN persona_id TEXT NOT NULL DEFAULT ''",
            [],
        );

        // Migration: recreate installed_skills with composite primary key (name, persona_id)
        // so the same skill can be installed for different personas.
        let has_composite_pk: bool = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='installed_skills'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|sql| sql.contains("PRIMARY KEY (name, persona_id)"))
            .unwrap_or(false);

        if !has_composite_pk {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS installed_skills_new (
                    name          TEXT NOT NULL,
                    source_id     TEXT NOT NULL,
                    source_path   TEXT NOT NULL,
                    local_path    TEXT NOT NULL,
                    manifest_json TEXT NOT NULL,
                    audit_json    TEXT NOT NULL,
                    enabled       INTEGER NOT NULL DEFAULT 1,
                    installed_at_ms INTEGER NOT NULL,
                    persona_id    TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (name, persona_id)
                );
                INSERT OR IGNORE INTO installed_skills_new
                    SELECT name, source_id, source_path, local_path, manifest_json,
                           audit_json, enabled, installed_at_ms, persona_id
                    FROM installed_skills;
                DROP TABLE installed_skills;
                ALTER TABLE installed_skills_new RENAME TO installed_skills;
                ",
            )
            .map_err(|e| IndexError::Database(e.to_string()))?;
        }

        // Migration: add content_hash and pinned_commit columns for integrity verification
        let _ = conn.execute(
            "ALTER TABLE installed_skills ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE installed_skills ADD COLUMN pinned_commit TEXT NOT NULL DEFAULT ''",
            [],
        );

        Ok(())
    }
}

impl SkillIndexStore for SqliteSkillIndex {
    fn save_source(&self, config: &SkillSourceConfig) -> Result<(), IndexError> {
        let conn = self.conn.lock();
        let json =
            serde_json::to_string(config).map_err(|e| IndexError::Serialization(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO skill_sources (source_id, config_json) VALUES (?1, ?2)",
            params![config.source_id(), json],
        )
        .map_err(|e| IndexError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_sources(&self) -> Result<Vec<SkillSourceConfig>, IndexError> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT config_json FROM skill_sources")
            .map_err(|e| IndexError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| IndexError::Database(e.to_string()))?;
        let mut configs = Vec::new();
        for row in rows {
            let json = row.map_err(|e| IndexError::Database(e.to_string()))?;
            let config: SkillSourceConfig = serde_json::from_str(&json)
                .map_err(|e| IndexError::Serialization(e.to_string()))?;
            configs.push(config);
        }
        Ok(configs)
    }

    fn clear_discovered(&self) -> Result<(), IndexError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM discovered_skills", [])
            .map_err(|e| IndexError::Database(e.to_string()))?;
        Ok(())
    }

    fn insert_discovered(&self, skills: &[DiscoveredSkill]) -> Result<(), IndexError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| IndexError::Database(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO discovered_skills
                     (name, source_id, source_path, description, manifest_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(|e| IndexError::Database(e.to_string()))?;

            for skill in skills {
                let manifest_json = serde_json::to_string(&skill.manifest)
                    .map_err(|e| IndexError::Serialization(e.to_string()))?;
                stmt.execute(params![
                    skill.manifest.name,
                    skill.source_id,
                    skill.source_path,
                    skill.manifest.description,
                    manifest_json,
                ])
                .map_err(|e| IndexError::Database(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| IndexError::Database(e.to_string()))?;
        Ok(())
    }

    fn list_discovered(
        &self,
        persona_id: Option<&str>,
    ) -> Result<Vec<DiscoveredSkill>, IndexError> {
        let conn = self.conn.lock();

        let (sql, bind_pid);
        match persona_id {
            Some(pid) => {
                sql = "SELECT d.name, d.source_id, d.source_path, d.manifest_json,
                              CASE WHEN i.name IS NOT NULL THEN 1 ELSE 0 END as installed
                       FROM discovered_skills d
                       LEFT JOIN installed_skills i ON d.name = i.name AND i.persona_id = ?1
                       ORDER BY d.name";
                bind_pid = Some(pid.to_string());
            }
            None => {
                sql = "SELECT d.name, d.source_id, d.source_path, d.manifest_json,
                              CASE WHEN i.name IS NOT NULL THEN 1 ELSE 0 END as installed
                       FROM discovered_skills d
                       LEFT JOIN installed_skills i ON d.name = i.name
                       ORDER BY d.name";
                bind_pid = None;
            }
        }

        let mut stmt = conn.prepare(sql).map_err(|e| IndexError::Database(e.to_string()))?;

        let rows = if let Some(ref pid) = bind_pid {
            stmt.query_map(params![pid], |row| {
                let manifest_json: String = row.get(3)?;
                let installed: bool = row.get(4)?;
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, manifest_json, installed))
            })
            .map_err(|e| IndexError::Database(e.to_string()))?
            .collect::<Vec<_>>()
        } else {
            stmt.query_map([], |row| {
                let manifest_json: String = row.get(3)?;
                let installed: bool = row.get(4)?;
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, manifest_json, installed))
            })
            .map_err(|e| IndexError::Database(e.to_string()))?
            .collect::<Vec<_>>()
        };

        let mut skills = Vec::new();
        for row in rows {
            let (source_id, source_path, manifest_json, installed) =
                row.map_err(|e| IndexError::Database(e.to_string()))?;
            let manifest: SkillManifest = serde_json::from_str(&manifest_json)
                .map_err(|e| IndexError::Serialization(e.to_string()))?;
            skills.push(DiscoveredSkill { manifest, source_id, source_path, installed });
        }
        Ok(skills)
    }

    fn install_skill(&self, skill: &InstalledSkill) -> Result<(), IndexError> {
        let conn = self.conn.lock();
        let manifest_json = serde_json::to_string(&skill.manifest)
            .map_err(|e| IndexError::Serialization(e.to_string()))?;
        let audit_json = serde_json::to_string(&skill.audit)
            .map_err(|e| IndexError::Serialization(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO installed_skills
             (name, source_id, source_path, local_path, manifest_json, audit_json, enabled, installed_at_ms, persona_id, content_hash, pinned_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                skill.manifest.name,
                skill.source_id,
                skill.source_path,
                skill.local_path,
                manifest_json,
                audit_json,
                skill.enabled as i32,
                skill.installed_at_ms,
                skill.persona_id,
                skill.content_hash,
                skill.pinned_commit,
            ],
        )
        .map_err(|e| IndexError::Database(e.to_string()))?;
        Ok(())
    }

    fn uninstall_skill(&self, name: &str, persona_id: Option<&str>) -> Result<bool, IndexError> {
        let conn = self.conn.lock();
        let count = match persona_id {
            Some(pid) => conn
                .execute(
                    "DELETE FROM installed_skills WHERE name = ?1 AND persona_id = ?2",
                    params![name, pid],
                )
                .map_err(|e| IndexError::Database(e.to_string()))?,
            None => conn
                .execute("DELETE FROM installed_skills WHERE name = ?1", params![name])
                .map_err(|e| IndexError::Database(e.to_string()))?,
        };
        Ok(count > 0)
    }

    fn set_skill_enabled(
        &self,
        name: &str,
        persona_id: Option<&str>,
        enabled: bool,
    ) -> Result<bool, IndexError> {
        let conn = self.conn.lock();
        let count = match persona_id {
            Some(pid) => conn
                .execute(
                    "UPDATE installed_skills SET enabled = ?1 WHERE name = ?2 AND persona_id = ?3",
                    params![enabled as i32, name, pid],
                )
                .map_err(|e| IndexError::Database(e.to_string()))?,
            None => conn
                .execute(
                    "UPDATE installed_skills SET enabled = ?1 WHERE name = ?2",
                    params![enabled as i32, name],
                )
                .map_err(|e| IndexError::Database(e.to_string()))?,
        };
        Ok(count > 0)
    }

    fn list_installed(&self, persona_id: Option<&str>) -> Result<Vec<InstalledSkill>, IndexError> {
        let conn = self.conn.lock();
        let (sql, filter_pid);
        match persona_id {
            Some(pid) => {
                sql = "SELECT name, source_id, source_path, local_path, manifest_json, audit_json, enabled, installed_at_ms, persona_id, content_hash, pinned_commit
                       FROM installed_skills WHERE persona_id = ?1 ORDER BY name";
                filter_pid = Some(pid.to_string());
            }
            None => {
                sql = "SELECT name, source_id, source_path, local_path, manifest_json, audit_json, enabled, installed_at_ms, persona_id, content_hash, pinned_commit
                       FROM installed_skills ORDER BY name";
                filter_pid = None;
            }
        }

        let mut stmt = conn.prepare(sql).map_err(|e| IndexError::Database(e.to_string()))?;

        let rows = if let Some(ref pid) = filter_pid {
            stmt.query_map(params![pid], |row| {
                Ok(InstalledRow {
                    source_id: row.get(1)?,
                    source_path: row.get(2)?,
                    local_path: row.get(3)?,
                    manifest_json: row.get(4)?,
                    audit_json: row.get(5)?,
                    enabled: row.get::<_, i32>(6)? != 0,
                    installed_at_ms: row.get(7)?,
                    persona_id: row.get(8)?,
                    content_hash: row.get(9)?,
                    pinned_commit: row.get(10)?,
                })
            })
            .map_err(|e| IndexError::Database(e.to_string()))?
            .collect::<Vec<_>>()
        } else {
            stmt.query_map([], |row| {
                Ok(InstalledRow {
                    source_id: row.get(1)?,
                    source_path: row.get(2)?,
                    local_path: row.get(3)?,
                    manifest_json: row.get(4)?,
                    audit_json: row.get(5)?,
                    enabled: row.get::<_, i32>(6)? != 0,
                    installed_at_ms: row.get(7)?,
                    persona_id: row.get(8)?,
                    content_hash: row.get(9)?,
                    pinned_commit: row.get(10)?,
                })
            })
            .map_err(|e| IndexError::Database(e.to_string()))?
            .collect::<Vec<_>>()
        };

        let mut skills = Vec::new();
        for row in rows {
            let r = row.map_err(|e| IndexError::Database(e.to_string()))?;
            let manifest: SkillManifest = serde_json::from_str(&r.manifest_json)
                .map_err(|e| IndexError::Serialization(e.to_string()))?;
            let audit: SkillAuditResult = serde_json::from_str(&r.audit_json)
                .map_err(|e| IndexError::Serialization(e.to_string()))?;
            skills.push(InstalledSkill {
                manifest,
                local_path: r.local_path,
                source_id: r.source_id,
                source_path: r.source_path,
                persona_id: r.persona_id,
                audit,
                enabled: r.enabled,
                installed_at_ms: r.installed_at_ms,
                content_hash: r.content_hash,
                pinned_commit: r.pinned_commit,
            });
        }
        Ok(skills)
    }

    fn get_installed(
        &self,
        name: &str,
        persona_id: Option<&str>,
    ) -> Result<Option<InstalledSkill>, IndexError> {
        let all = self.list_installed(persona_id)?;
        Ok(all.into_iter().find(|s| s.manifest.name == name))
    }

    fn list_enabled(&self, persona_id: Option<&str>) -> Result<Vec<InstalledSkill>, IndexError> {
        Ok(self.list_installed(persona_id)?.into_iter().filter(|s| s.enabled).collect())
    }
}

struct InstalledRow {
    source_id: String,
    source_path: String,
    local_path: String,
    manifest_json: String,
    audit_json: String,
    enabled: bool,
    installed_at_ms: u64,
    persona_id: String,
    content_hash: String,
    pinned_commit: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("database error: {0}")]
    Database(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("I/O error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::{SkillAuditResult, SkillManifest};

    fn test_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: format!("Test skill: {name}"),
            license: None,
            compatibility: None,
            metadata: Default::default(),
            allowed_tools: None,
        }
    }

    fn test_audit() -> SkillAuditResult {
        SkillAuditResult {
            model_used: "test-model".to_string(),
            risks: vec![],
            summary: "No risks found.".to_string(),
            audited_at_ms: 1000,
        }
    }

    #[test]
    fn roundtrip_installed_skill() {
        let index = SkillIndex::in_memory().unwrap();
        let skill = InstalledSkill {
            manifest: test_manifest("test-skill"),
            local_path: "/tmp/skills/test-skill".to_string(),
            source_id: "github:test/repo".to_string(),
            source_path: "skills/test-skill".to_string(),
            persona_id: String::new(),
            audit: test_audit(),
            enabled: true,
            installed_at_ms: 12345,
            content_hash: "abc123".to_string(),
            pinned_commit: "def456".to_string(),
        };
        index.install_skill(&skill).unwrap();

        let installed = index.list_installed(None).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].manifest.name, "test-skill");
        assert!(installed[0].enabled);

        // Disable
        index.set_skill_enabled("test-skill", None, false).unwrap();
        let enabled = index.list_enabled(None).unwrap();
        assert!(enabled.is_empty());

        // Uninstall
        assert!(index.uninstall_skill("test-skill", None).unwrap());
        assert!(index.list_installed(None).unwrap().is_empty());
    }

    #[test]
    fn discovered_marks_installed() {
        let index = SkillIndex::in_memory().unwrap();

        let discovered = vec![
            DiscoveredSkill {
                manifest: test_manifest("skill-a"),
                source_id: "github:test/repo".to_string(),
                source_path: "skills/skill-a".to_string(),
                installed: false,
            },
            DiscoveredSkill {
                manifest: test_manifest("skill-b"),
                source_id: "github:test/repo".to_string(),
                source_path: "skills/skill-b".to_string(),
                installed: false,
            },
        ];
        index.insert_discovered(&discovered).unwrap();

        // Install skill-a
        index
            .install_skill(&InstalledSkill {
                manifest: test_manifest("skill-a"),
                local_path: "/tmp/skills/skill-a".to_string(),
                source_id: "github:test/repo".to_string(),
                source_path: "skills/skill-a".to_string(),
                persona_id: String::new(),
                audit: test_audit(),
                enabled: true,
                installed_at_ms: 1000,
                content_hash: String::new(),
                pinned_commit: String::new(),
            })
            .unwrap();

        let listed = index.list_discovered(None).unwrap();
        assert_eq!(listed.len(), 2);
        let a = listed.iter().find(|s| s.manifest.name == "skill-a").unwrap();
        let b = listed.iter().find(|s| s.manifest.name == "skill-b").unwrap();
        assert!(a.installed);
        assert!(!b.installed);
    }
}
