use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use crate::services::communication::{
    CommAttachment, CommunicationService, InboundMessage, RichMessageBody,
};

use super::graph_client::GraphClient;

/// Microsoft 365 Mail communication service backed by Graph API.
pub struct MicrosoftCommunication {
    graph: Arc<GraphClient>,
    from_address: String,
    folder: String,
}

impl MicrosoftCommunication {
    pub fn new(graph: Arc<GraphClient>, from_address: &str, folder: &str) -> Self {
        Self { graph, from_address: from_address.to_string(), folder: folder.to_string() }
    }
}

#[async_trait]
impl CommunicationService for MicrosoftCommunication {
    fn name(&self) -> &str {
        "Microsoft 365 Mail"
    }

    async fn test_connection(&self) -> Result<()> {
        let path = format!("/me/mailFolders/{}", urlencoding::encode(&self.folder));
        self.graph.get(&path).await?;
        info!(connector = %self.graph.connector_id(), "Graph API mail connection test OK");
        Ok(())
    }

    async fn send(
        &self,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let recipients: Vec<serde_json::Value> = to
            .iter()
            .map(|addr| {
                serde_json::json!({
                    "emailAddress": { "address": addr }
                })
            })
            .collect();

        let mut message = serde_json::json!({
            "subject": subject.unwrap_or(""),
            "body": {
                "contentType": "Text",
                "content": body
            },
            "toRecipients": recipients,
        });

        // Only include `from` if a from_address is explicitly configured;
        // otherwise let Graph use the authenticated user's default address.
        if !self.from_address.is_empty() {
            message["from"] = serde_json::json!({
                "emailAddress": { "address": &self.from_address }
            });
        }

        if !attachments.is_empty() {
            // Include file attachments inline in the message payload
            let att_array: Vec<serde_json::Value> = attachments
                .iter()
                .map(|att| {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&att.data);
                    serde_json::json!({
                        "@odata.type": "#microsoft.graph.fileAttachment",
                        "name": att.filename,
                        "contentType": att.media_type,
                        "contentBytes": b64,
                    })
                })
                .collect();
            message["attachments"] = serde_json::json!(att_array);
        }

        let payload = serde_json::json!({ "message": message });

        let resp = self
            .graph
            .post("/me/sendMail", &payload)
            .await
            .context("Graph API sendMail request failed")?;

        if resp.status().is_success() {
            info!(connector = %self.graph.connector_id(), to = ?to, "email sent via Graph API");
            Ok("sent-via-graph".to_string())
        } else {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            bail!("Graph API sendMail failed ({status}): {err_body}");
        }
    }

    async fn send_rich(
        &self,
        to: &[String],
        subject: Option<&str>,
        fallback_text: &str,
        rich_body: Option<RichMessageBody>,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let html = match &rich_body {
            Some(RichMessageBody::Html(h)) => h.clone(),
            _ => return self.send(to, subject, fallback_text, attachments).await,
        };

        let recipients: Vec<serde_json::Value> = to
            .iter()
            .map(|addr| {
                serde_json::json!({
                    "emailAddress": { "address": addr }
                })
            })
            .collect();

        let mut message = serde_json::json!({
            "subject": subject.unwrap_or(""),
            "body": {
                "contentType": "HTML",
                "content": html
            },
            "toRecipients": recipients,
        });

        if !self.from_address.is_empty() {
            message["from"] = serde_json::json!({
                "emailAddress": { "address": &self.from_address }
            });
        }

        if !attachments.is_empty() {
            let att_array: Vec<serde_json::Value> = attachments
                .iter()
                .map(|att| {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&att.data);
                    serde_json::json!({
                        "@odata.type": "#microsoft.graph.fileAttachment",
                        "name": att.filename,
                        "contentType": att.media_type,
                        "contentBytes": b64,
                    })
                })
                .collect();
            message["attachments"] = serde_json::json!(att_array);
        }

        let payload = serde_json::json!({ "message": message });

        let resp = self
            .graph
            .post("/me/sendMail", &payload)
            .await
            .context("Graph API sendMail (HTML) request failed")?;

        if resp.status().is_success() {
            info!(connector = %self.graph.connector_id(), to = ?to, "HTML email sent via Graph API");
            Ok("sent-via-graph".to_string())
        } else {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            bail!("Graph API sendMail (HTML) failed ({status}): {err_body}");
        }
    }

    async fn fetch_new(&self, limit: usize) -> Result<Vec<InboundMessage>> {
        let path = format!(
            "/me/mailFolders/{}/messages?\
             $filter=isRead eq false\
             &$top={limit}\
             &$orderby=receivedDateTime desc\
             &$select=id,from,toRecipients,subject,body,receivedDateTime,internetMessageHeaders,hasAttachments\
             &$expand=attachments($select=id,name,contentType,size)",
            urlencoding::encode(&self.folder),
        );

        let body = self.graph.get(&path).await?;
        let items = body["value"].as_array().cloned().unwrap_or_default();

        let mut messages = Vec::new();
        for item in &items {
            let msg_id = item["id"].as_str().unwrap_or_default().to_string();

            let from =
                item["from"]["emailAddress"]["address"].as_str().unwrap_or("unknown").to_string();

            let to_addrs: Vec<String> = item["toRecipients"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| r["emailAddress"]["address"].as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            let subject = item["subject"].as_str().map(|s| s.to_string());
            let body_content = item["body"]["content"].as_str().unwrap_or_default().to_string();

            let received = item["receivedDateTime"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis() as u128)
                .unwrap_or_else(|| {
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
                });

            let mut metadata = HashMap::new();
            metadata.insert("graph_message_id".to_string(), msg_id.clone());
            metadata.insert("folder".to_string(), self.folder.clone());

            // Extract reply-tracking headers from internetMessageHeaders.
            if let Some(headers) = item["internetMessageHeaders"].as_array() {
                for hdr in headers {
                    let name = hdr["name"].as_str().unwrap_or("").to_lowercase();
                    let value = hdr["value"].as_str().unwrap_or("");
                    match name.as_str() {
                        "in-reply-to" => {
                            metadata.insert("in_reply_to".to_string(), value.to_string());
                        }
                        "references" => {
                            metadata.insert("references".to_string(), value.to_string());
                        }
                        "message-id" => {
                            metadata.insert("message_id".to_string(), value.to_string());
                        }
                        _ => {}
                    }
                }
            }

            // Extract [HIVEMIND:<token>] from subject for reply matching.
            if let Some(ref subj) = subject {
                if let Some(start) = subj.find("[HIVEMIND:") {
                    if let Some(end) = subj[start..].find(']') {
                        let token = &subj[start + 7..start + end];
                        metadata.insert("hivemind_token".to_string(), token.to_string());
                    }
                }
            }

            // Parse attachment metadata from the expanded response.
            let attachments: Vec<CommAttachment> = item["attachments"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|att| {
                            let id = att["id"].as_str()?.to_string();
                            let filename = att["name"].as_str().unwrap_or("attachment").to_string();
                            let media_type = att["contentType"]
                                .as_str()
                                .unwrap_or("application/octet-stream")
                                .to_string();
                            Some(CommAttachment {
                                id: Some(id),
                                filename,
                                media_type,
                                data: Vec::new(), // metadata only; use download_attachment for content
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            messages.push(InboundMessage {
                external_id: format!("{}:graph:{}", self.graph.connector_id(), msg_id),
                from,
                to: to_addrs,
                subject,
                body: body_content,
                timestamp_ms: received,
                metadata,
                attachments,
            });
        }

        debug!(
            connector = %self.graph.connector_id(),
            count = messages.len(),
            "fetched emails via Graph API"
        );
        Ok(messages)
    }

    async fn mark_seen(&self, message_id: &str) -> Result<()> {
        // external_id is "connector_id:graph:graph_message_id"
        let graph_id = message_id.rsplit_once(":graph:").map(|(_, id)| id).unwrap_or(message_id);

        let resp = self
            .graph
            .patch(
                &format!("/me/messages/{}", urlencoding::encode(graph_id)),
                &serde_json::json!({ "isRead": true }),
            )
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            bail!("Graph API mark_seen failed ({status}): {err_body}");
        }

        debug!(
            connector = %self.graph.connector_id(),
            message_id,
            "marked message as read via Graph API"
        );
        Ok(())
    }

    async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<CommAttachment> {
        // message_id may be a full external_id (connector_id:graph:graph_msg_id) or raw graph id
        let graph_msg_id =
            message_id.rsplit_once(":graph:").map(|(_, id)| id).unwrap_or(message_id);

        let path = format!(
            "/me/messages/{}/attachments/{}",
            urlencoding::encode(graph_msg_id),
            urlencoding::encode(attachment_id),
        );

        let body = self.graph.get(&path).await.context("Graph API download attachment failed")?;

        let filename = body["name"].as_str().unwrap_or("attachment").to_string();
        let media_type =
            body["contentType"].as_str().unwrap_or("application/octet-stream").to_string();

        // Graph API returns contentBytes as standard base64
        let content_b64 = body["contentBytes"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("attachment response missing contentBytes"))?;

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .context("failed to decode attachment contentBytes")?;

        Ok(CommAttachment { id: Some(attachment_id.to_string()), filename, media_type, data })
    }
}
