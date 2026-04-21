//! hive-skills — Agent Skills discovery, auditing, and management.
//!
//! Implements the [Agent Skills](https://agentskills.io/specification) open standard.

pub mod catalog;
pub mod github_source;
pub mod index;
pub mod local_dir_source;
pub mod parser;
pub mod scan;

pub use catalog::{stage_skill_resources, SkillCatalog};
pub use github_source::GitHubRepoSource;
pub use index::{SkillIndex, SkillIndexStore, SqliteSkillIndex};
pub use local_dir_source::LocalDirSource;
pub use parser::{parse_skill_md, ParsedSkill};
