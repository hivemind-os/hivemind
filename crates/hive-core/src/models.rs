//! Shared model-related DTOs used by both the API and desktop frontends.
//! These are pure data types with no heavy runtime dependencies.
//! Types are now defined in hive-contracts; re-exported here for backward compatibility.

pub use hive_contracts::{
    HubModelInfo, HubSearchResult, InstalledModel, ModelCapabilities, ModelStatus,
};
