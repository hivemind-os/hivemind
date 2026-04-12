use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::Contact;
use tracing::info;

use super::google_client::GoogleClient;
use crate::services::ContactsService;

const PEOPLE_API: &str = "https://people.googleapis.com/v1";

const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations";

pub struct GoogleContacts {
    google: Arc<GoogleClient>,
    default_class: DataClass,
}

impl GoogleContacts {
    pub fn new(google: Arc<GoogleClient>, default_class: DataClass) -> Self {
        Self { google, default_class }
    }

    fn parse_contact(&self, person: &serde_json::Value) -> Contact {
        let display_name = person["names"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|n| n["displayName"].as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                person["emailAddresses"]
                    .as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|e| e["value"].as_str())
            })
            .unwrap_or("Unknown")
            .to_string();

        let first_org = person["organizations"].as_array().and_then(|arr| arr.first());

        Contact {
            id: person["resourceName"].as_str().unwrap_or("").to_string(),
            connector_id: self.google.connector_id().to_string(),
            display_name,
            email_addresses: person["emailAddresses"]
                .as_array()
                .map(|arr| {
                    arr.iter().filter_map(|e| e["value"].as_str()).map(|s| s.to_string()).collect()
                })
                .unwrap_or_default(),
            phone_numbers: person["phoneNumbers"]
                .as_array()
                .map(|arr| {
                    arr.iter().filter_map(|p| p["value"].as_str()).map(|s| s.to_string()).collect()
                })
                .unwrap_or_default(),
            company: first_org
                .and_then(|o| o["name"].as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            job_title: first_org
                .and_then(|o| o["title"].as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            data_class: self.default_class,
        }
    }
}

#[async_trait]
impl ContactsService for GoogleContacts {
    fn name(&self) -> &str {
        "Google Contacts"
    }

    async fn test_connection(&self) -> Result<()> {
        let url = format!("{PEOPLE_API}/people/me?personFields=names");
        self.google.get(&url).await?;
        info!(
            connector = %self.google.connector_id(),
            "Contacts connection test OK"
        );
        Ok(())
    }

    async fn list_contacts(&self, limit: usize, offset: usize) -> Result<Vec<Contact>> {
        let page_size = limit + offset;
        let url = format!(
            "{PEOPLE_API}/people/me/connections\
             ?personFields={PERSON_FIELDS}\
             &pageSize={page_size}\
             &sortOrder=FIRST_NAME_ASCENDING"
        );
        let body = self.google.get(&url).await?;
        let items = body["connections"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().skip(offset).map(|p| self.parse_contact(p)).collect())
    }

    async fn search_contacts(&self, query: &str, limit: usize) -> Result<Vec<Contact>> {
        let encoded_query = urlencoding::encode(query);
        let url = format!(
            "{PEOPLE_API}/people:searchContacts\
             ?query={encoded_query}\
             &readMask={PERSON_FIELDS}\
             &pageSize={limit}"
        );
        match self.google.get(&url).await {
            Ok(body) => {
                let results = body["results"].as_array().cloned().unwrap_or_default();
                Ok(results
                    .iter()
                    .filter_map(|r| r.get("person"))
                    .map(|p| self.parse_contact(p))
                    .collect())
            }
            Err(_) => {
                // Fallback: fetch a batch and filter client-side
                let url = format!(
                    "{PEOPLE_API}/people/me/connections\
                     ?personFields={PERSON_FIELDS}\
                     &pageSize=200\
                     &sortOrder=FIRST_NAME_ASCENDING"
                );
                let body = self.google.get(&url).await?;
                let items = body["connections"].as_array().cloned().unwrap_or_default();
                let query_lower = query.to_lowercase();
                let filtered: Vec<Contact> = items
                    .iter()
                    .map(|p| self.parse_contact(p))
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
        let resource_name = if contact_id.starts_with("people/") {
            contact_id.to_string()
        } else {
            format!("people/{contact_id}")
        };
        let url = format!("{PEOPLE_API}/{resource_name}?personFields={PERSON_FIELDS}");
        let body = self.google.get(&url).await?;
        if body["resourceName"].as_str().unwrap_or("").is_empty() {
            bail!("contact not found: {contact_id}");
        }
        Ok(self.parse_contact(&body))
    }
}
