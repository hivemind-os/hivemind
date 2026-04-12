use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::Contact;
use tracing::info;

use super::graph_client::GraphClient;
use crate::services::ContactsService;

pub struct MicrosoftContacts {
    graph: Arc<GraphClient>,
    default_class: DataClass,
}

impl MicrosoftContacts {
    pub fn new(graph: Arc<GraphClient>, default_class: DataClass) -> Self {
        Self { graph, default_class }
    }

    fn parse_contact(&self, item: &serde_json::Value) -> Contact {
        Contact {
            id: item["id"].as_str().unwrap_or("").to_string(),
            connector_id: self.graph.connector_id().to_string(),
            display_name: item["displayName"].as_str().unwrap_or("").to_string(),
            email_addresses: item["emailAddresses"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| e["address"].as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            phone_numbers: item["phones"]
                .as_array()
                .map(|arr| {
                    arr.iter().filter_map(|p| p["number"].as_str()).map(|s| s.to_string()).collect()
                })
                .unwrap_or_default(),
            company: item["companyName"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            job_title: item["jobTitle"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            data_class: self.default_class,
        }
    }
}

#[async_trait]
impl ContactsService for MicrosoftContacts {
    fn name(&self) -> &str {
        "Microsoft 365 Contacts"
    }

    async fn test_connection(&self) -> Result<()> {
        self.graph.get("/me/contacts?$top=1").await?;
        info!(
            connector = %self.graph.connector_id(),
            "Contacts connection test OK"
        );
        Ok(())
    }

    async fn list_contacts(&self, limit: usize, offset: usize) -> Result<Vec<Contact>> {
        let path = format!(
            "/me/contacts?$top={limit}&$skip={offset}\
             &$select=id,displayName,emailAddresses,phones,companyName,jobTitle\
             &$orderby=displayName"
        );
        let body = self.graph.get(&path).await?;
        let items = body["value"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|i| self.parse_contact(i)).collect())
    }

    async fn search_contacts(&self, query: &str, limit: usize) -> Result<Vec<Contact>> {
        // Graph $filter on contacts is limited; fall back to client-side filtering
        // if the OData filter fails.
        let filter_path = format!(
            "/me/contacts?$filter=contains(displayName,'{query}')\
             &$top={limit}\
             &$select=id,displayName,emailAddresses,phones,companyName,jobTitle"
        );
        match self.graph.get(&filter_path).await {
            Ok(body) => {
                let items = body["value"].as_array().cloned().unwrap_or_default();
                Ok(items.iter().map(|i| self.parse_contact(i)).collect())
            }
            Err(_) => {
                // Fallback: fetch a batch and filter client-side
                let path = "/me/contacts?$top=200\
                     &$select=id,displayName,emailAddresses,phones,companyName,jobTitle"
                    .to_string();
                let body = self.graph.get(&path).await?;
                let items = body["value"].as_array().cloned().unwrap_or_default();
                let query_lower = query.to_lowercase();
                let filtered: Vec<Contact> = items
                    .iter()
                    .map(|i| self.parse_contact(i))
                    .filter(|c| {
                        c.display_name.to_lowercase().contains(&query_lower)
                            || c.email_addresses
                                .iter()
                                .any(|e| e.to_lowercase().contains(&query_lower))
                    })
                    .take(limit)
                    .collect();
                Ok(filtered)
            }
        }
    }

    async fn get_contact(&self, contact_id: &str) -> Result<Contact> {
        let path = format!(
            "/me/contacts/{contact_id}\
             ?$select=id,displayName,emailAddresses,phones,companyName,jobTitle"
        );
        let body = self.graph.get(&path).await?;
        Ok(self.parse_contact(&body))
    }
}
