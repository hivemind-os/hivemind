//! hive-skills — Agent Skills discovery, auditing, and management.
//!
//! Implements the [Agent Skills](https://agentskills.io/specification) open standard.

pub mod catalog;
pub mod github_source;
pub mod index;
pub mod parser;

pub use catalog::{SkillCatalog, stage_skill_resources};
pub use github_source::GitHubRepoSource;
pub use index::{SkillIndex, SkillIndexStore, SqliteSkillIndex};
pub use parser::{parse_skill_md, ParsedSkill};
