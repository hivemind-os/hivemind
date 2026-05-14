//! # hive-workflow
//!
//! Workflow definition, validation, analysis, and execution support for HiveMind OS.
//! This crate models workflow graphs, resolves expressions, persists definitions and
//! instances, and provides both live and shadow execution paths for automation.
//!
//! ## Key exports
//!
//! - [`WorkflowDefinition`] and related items from [`types`] — the workflow schema surface.
//! - [`WorkflowEngine`] and [`ExecutionContext`] — execute workflow instances step by step.
//! - [`SqliteWorkflowStore`] and [`WorkflowStore`] — persist workflow definitions and runs.
//! - [`analyze_workflow`] and [`validate_definition`] — inspect risk and validate authoring input.
//!
//! ## Crate relationships
//!
//! Depends on `hive-contracts` for shared workflow-facing types and is consumed by
//! tooling and services such as `hive-tools` and `hive-workflow-service`.
//!
//! ## Usage notes
//!
//! Validate and analyze definitions before launch, and use the shadow executor or test runner for non-destructive workflow checks.

pub mod analyzer;
pub mod attachments;
pub mod catalog;
pub mod error;
pub mod executor;
pub mod expression;
pub mod shadow_executor;
pub mod store;
pub mod test_runner;
pub mod types;
pub mod validation;

pub use analyzer::*;
pub use attachments::*;
pub use catalog::*;
pub use error::*;
pub use executor::*;
pub use expression::*;
pub use shadow_executor::*;
pub use store::*;
pub use test_runner::*;
pub use types::*;
pub use validation::*;
