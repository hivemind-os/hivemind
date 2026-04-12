pub mod audit;
pub mod bundled;
pub mod config;
pub mod daemon_control;
pub mod daemon_token;
pub mod entity_graph;
pub mod event_bus;
pub mod event_log;
pub mod model_limits;
pub mod models;
pub mod prompt_template;
pub mod secret_cache;
pub mod secret_store;
pub mod service_log;
pub mod service_manager;

// Re-export all contract types that were previously defined here
pub use hive_contracts::{
    ApiConfig, CapabilityConfig, DaemonConfig, DaemonStatus, HiveMindConfig, HiveMindPaths,
    HubModelInfo, HubSearchResult, InferenceRuntimeKind, InstalledModel, LocalModelsConfig,
    McpHeaderValue, McpServerConfig, McpTransportConfig, ModelCapabilities, ModelProviderConfig,
    ModelStatus, ModelTask, ModelsConfig, OverridePolicyConfig, PolicyAction,
    PromptInjectionConfig, PromptTemplate, ProviderAuthConfig, ProviderKindConfig,
    ProviderOptionsConfig, ScannerAction, ScannerModelEntry, SecurityConfig,
};

// Re-export config operation functions
pub use config::{
    archive_persona, config_to_yaml, discover_paths, ensure_paths, hivemind_paths_from,
    load_config, load_config_from_paths, load_config_with_cwd, load_personas,
    migrate_personas_from_config, save_config, save_persona, validate_config, validate_config_file,
};

pub use bundled::{
    bundled_persona_skill_names, bundled_persona_yaml, bundled_persona_yamls,
    bundled_workflow_yaml, bundled_workflow_yamls, is_bundled_persona, is_bundled_workflow,
    reset_bundled_persona, seed_bundled_personas,
};

pub use audit::{AuditEntry, AuditLogger, NewAuditEntry};
pub use daemon_control::{
    daemon_start, daemon_status, daemon_stop, daemon_url, request_apple_access,
    resolve_daemon_binary, AppleAccessResult,
};
pub use entity_graph::{
    agent_ref, parse_entity_ref, session_ref, workflow_ref, EntityGraph, EntityNode, EntityRef,
    EntityType,
};
pub use event_bus::{EventBus, EventEnvelope, QueuedSubscriber, TopicSubscription};
pub use event_log::{
    EventLog, EventLogStore, RecordedEvent, Recording, RecordingSummary, SqliteEventLogStore,
    StoredEvent,
};
pub use model_limits::{ModelLimits, ModelLimitsRegistry, ModelMetadata, ModelMetadataRegistry};
pub use prompt_template::{find_prompt_template, render_prompt_template};
pub use service_log::{LogEntry, LogQuery, ServiceLogCollector};
pub use service_manager::{service_load, service_status, service_unload, ServiceStatus};
