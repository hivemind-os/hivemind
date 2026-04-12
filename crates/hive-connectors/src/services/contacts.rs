use async_trait::async_trait;
use hive_contracts::connectors::Contact;

/// Abstract interface for the Contacts service on a connector.
#[async_trait]
pub trait ContactsService: Send + Sync {
    /// Human-readable name (e.g. "Microsoft 365 Contacts").
    fn name(&self) -> &str;

    /// Test contacts connectivity and authentication.
    async fn test_connection(&self) -> anyhow::Result<()>;

    /// List contacts with pagination.
    async fn list_contacts(&self, limit: usize, offset: usize) -> anyhow::Result<Vec<Contact>>;

    /// Search contacts by query string (name, email, etc.).
    async fn search_contacts(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Contact>>;

    /// Get a single contact by ID.
    async fn get_contact(&self, contact_id: &str) -> anyhow::Result<Contact>;
}
