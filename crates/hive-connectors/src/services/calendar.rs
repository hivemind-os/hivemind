use async_trait::async_trait;
use hive_contracts::connectors::{CalendarEvent, EventUpdate, FreeBusySlot, NewCalendarEvent};

/// Abstract interface for the Calendar service on a connector.
#[async_trait]
pub trait CalendarService: Send + Sync {
    /// Human-readable name (e.g. "Microsoft 365 Calendar").
    fn name(&self) -> &str;

    /// Test calendar connectivity and authentication.
    async fn test_connection(&self) -> anyhow::Result<()>;

    /// List events within a time range.
    ///
    /// `start` and `end` are ISO 8601 timestamps.
    async fn list_events(&self, start: &str, end: &str) -> anyhow::Result<Vec<CalendarEvent>>;

    /// Get a single event by ID.
    async fn get_event(&self, event_id: &str) -> anyhow::Result<CalendarEvent>;

    /// Create a new calendar event.
    async fn create_event(&self, event: &NewCalendarEvent) -> anyhow::Result<CalendarEvent>;

    /// Update an existing event.
    async fn update_event(
        &self,
        event_id: &str,
        update: &EventUpdate,
    ) -> anyhow::Result<CalendarEvent>;

    /// Delete an event.
    async fn delete_event(&self, event_id: &str) -> anyhow::Result<()>;

    /// Check free/busy availability within a time range.
    async fn check_availability(&self, start: &str, end: &str)
        -> anyhow::Result<Vec<FreeBusySlot>>;
}
