//! A mock connector for integration tests.
//!
//! `MockConnector` implements [`Connector`] and [`CommunicationService`],
//! providing controllable email send/receive behaviour.  Outbound messages
//! are recorded for post-test assertions; inbound messages can be scripted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hive_connectors::services::communication::{
    ChannelInfo, CommAttachment, CommunicationService, InboundMessage,
};
use hive_connectors::Connector;
use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

/// A recorded outbound message sent through the mock connector.
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub to: Vec<String>,
    pub subject: Option<String>,
    pub body: String,
    pub attachments: Vec<String>, // filenames only
}

struct MockConnectorState {
    inbound: Vec<InboundMessage>,
    sent: Vec<SentMessage>,
    seen: Vec<String>,
}

/// A mock connector that records sends and returns scripted inbound messages.
pub struct MockConnector {
    id: String,
    state: Arc<Mutex<MockConnectorState>>,
}

impl MockConnector {
    /// Create a new mock connector with the given ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: Arc::new(Mutex::new(MockConnectorState {
                inbound: Vec::new(),
                sent: Vec::new(),
                seen: Vec::new(),
            })),
        }
    }

    /// Pre-load inbound messages that `fetch_new()` will return.
    pub fn with_inbound(self, messages: Vec<InboundMessage>) -> Self {
        self.state.lock().expect("poisoned").inbound = messages;
        self
    }

    /// Return all recorded outbound messages.
    pub fn sent_messages(&self) -> Vec<SentMessage> {
        self.state.lock().expect("poisoned").sent.clone()
    }

    /// Return all message IDs that were marked as seen.
    pub fn seen_message_ids(&self) -> Vec<String> {
        self.state.lock().expect("poisoned").seen.clone()
    }

    /// Helper to build an `InboundMessage` for testing.
    pub fn make_email(
        external_id: &str,
        from: &str,
        to: &[&str],
        subject: &str,
        body: &str,
    ) -> InboundMessage {
        InboundMessage {
            external_id: external_id.to_string(),
            from: from.to_string(),
            to: to.iter().map(|s| s.to_string()).collect(),
            subject: Some(subject.to_string()),
            body: body.to_string(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            metadata: HashMap::new(),
            attachments: Vec::new(),
        }
    }
}

impl Connector for MockConnector {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        "Mock Email"
    }

    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Imap
    }

    fn enabled_services(&self) -> Vec<ServiceType> {
        vec![ServiceType::Communication]
    }

    fn status(&self) -> ConnectorStatus {
        ConnectorStatus::Connected
    }

    fn communication(&self) -> Option<&dyn CommunicationService> {
        Some(self)
    }

    fn calendar(&self) -> Option<&dyn hive_connectors::services::CalendarService> {
        None
    }

    fn drive(&self) -> Option<&dyn hive_connectors::services::DriveService> {
        None
    }

    fn contacts(&self) -> Option<&dyn hive_connectors::services::ContactsService> {
        None
    }
}

#[async_trait]
impl CommunicationService for MockConnector {
    fn name(&self) -> &str {
        "Mock Email Service"
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(
        &self,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> anyhow::Result<String> {
        let msg_id = format!("mock-msg-{}", uuid::Uuid::new_v4());
        self.state.lock().expect("poisoned").sent.push(SentMessage {
            to: to.to_vec(),
            subject: subject.map(String::from),
            body: body.to_string(),
            attachments: attachments.iter().map(|a| a.filename.clone()).collect(),
        });
        Ok(msg_id)
    }

    async fn fetch_new(&self, limit: usize) -> anyhow::Result<Vec<InboundMessage>> {
        let mut state = self.state.lock().expect("poisoned");
        let count = limit.min(state.inbound.len());
        Ok(state.inbound.drain(..count).collect())
    }

    async fn mark_seen(&self, message_id: &str) -> anyhow::Result<()> {
        self.state.lock().expect("poisoned").seen.push(message_id.to_string());
        Ok(())
    }

    fn supports_idle(&self) -> bool {
        false
    }

    async fn wait_for_changes(&self, _timeout: Duration) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn list_channels(&self) -> anyhow::Result<Vec<ChannelInfo>> {
        Ok(vec![ChannelInfo {
            id: self.id.clone(),
            name: "Mock Inbox".into(),
            channel_type: Some("folder".into()),
            group_name: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_sent_messages() {
        let connector = MockConnector::new("test-email");

        let msg_id = connector
            .send(&["user@example.com".to_string()], Some("Hello"), "Hi there", &[])
            .await
            .unwrap();

        assert!(msg_id.starts_with("mock-msg-"));
        let sent = connector.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, vec!["user@example.com"]);
        assert_eq!(sent[0].subject.as_deref(), Some("Hello"));
        assert_eq!(sent[0].body, "Hi there");
    }

    #[tokio::test]
    async fn returns_scripted_inbound() {
        let connector =
            MockConnector::new("test-email").with_inbound(vec![MockConnector::make_email(
                "e1",
                "sender@test.com",
                &["me@test.com"],
                "Test",
                "Body",
            )]);

        let messages = connector.fetch_new(10).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, "sender@test.com");

        // Second fetch should be empty.
        let messages2 = connector.fetch_new(10).await.unwrap();
        assert!(messages2.is_empty());
    }
}
