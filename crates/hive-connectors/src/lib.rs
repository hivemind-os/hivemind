pub mod adapters;
pub mod audit;
pub mod config;
pub mod connector;
pub mod mail_utils;
pub mod message_state;
pub mod providers;
pub mod registry;
pub mod resolver;
pub mod secrets;
pub mod service;
pub mod service_registry;
pub mod services;

/// Spawn a blocking closure while propagating the current tracing span.
///
/// `tokio::task::spawn_blocking` does not automatically carry the current
/// tracing span into the blocking thread.  This helper captures
/// `Span::current()` before spawning and enters it inside the closure so
/// that log events emitted on the blocking thread are attributed to the
/// correct service span (important for `ServiceLogCollector`).
pub fn spawn_blocking_with_span<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || {
        let _guard = span.enter();
        f()
    })
}

pub use adapters::{
    CalendarServiceAdapter, CommunicationServiceAdapter, ContactsServiceAdapter,
    DriveServiceAdapter,
};
pub use audit::{AuditStore, ConnectorAuditLog, SqliteAuditStore};
pub use config::{ConnectorConfig, SmtpEncryption};
pub use connector::{Connector, InboundMessage};
pub use message_state::{MessageState, MessageStateStore, SqliteMessageStateStore};
pub use registry::ConnectorRegistry;
pub use resolver::ResourceResolver;
pub use service::{ConnectorService, ConnectorServiceHandle, PollHealth};
pub use service_registry::{DynService, OperationSchema, ServiceDescriptor, ServiceRegistry};
pub use services::{CalendarService, CommunicationService, ContactsService, DriveService};
