use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use tracing::{debug, info};

use super::google_client::GoogleClient;
use crate::services::communication::{
    ChannelInfo, CommAttachment, CommunicationService, InboundMessage, RichMessageBody,
};

const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

/// Gmail communication service backed by the Gmail REST API.
///
/// Uses `GoogleClient` for authenticated HTTP requests with automatic
/// OAuth2 token refresh.
pub struct GmailCommunication {
    google: Arc<GoogleClient>,
    from_address: String,
    folder: String,
    connector_id: String,
}

impl GmailCommunication {
    pub fn new(
        google: Arc<GoogleClient>,
        connector_id: &str,
        from_address: &str,
        folder: &str,
    ) -> Self {
        Self {
            google,
            from_address: from_address.to_string(),
            folder: folder.to_string(),
            connector_id: connector_id.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// RFC 2822 message building helpers
// ---------------------------------------------------------------------------

fn b64url() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// Build a simple single-part RFC 2822 message.
fn build_rfc2822_simple(
    from: &str,
    to: &[String],
    subject: &str,
    body: &str,
    content_type: &str,
) -> String {
    format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: {content_type}\r\n\
         \r\n\
         {body}",
        to = to.join(", "),
    )
}

/// Build a multipart/mixed RFC 2822 message with attachments.
fn build_rfc2822_with_attachments(
    from: &str,
    to: &[String],
    subject: &str,
    body: &str,
    content_type: &str,
    attachments: &[CommAttachment],
) -> String {
    let boundary = format!("hivemind_boundary_{}", uuid_v4_simple());
    let mut msg = format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\
         \r\n\
         --{boundary}\r\n\
         Content-Type: {content_type}\r\n\
         \r\n\
         {body}\r\n",
        to = to.join(", "),
    );

    for att in attachments {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&att.data);
        msg.push_str(&format!(
            "--{boundary}\r\n\
             Content-Type: {media_type}; name=\"{filename}\"\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             {encoded}\r\n",
            media_type = att.media_type,
            filename = att.filename,
        ));
    }

    msg.push_str(&format!("--{boundary}--\r\n"));
    msg
}

/// Build a multipart/alternative message (text + HTML), optionally wrapped in
/// multipart/mixed when attachments are present.
fn build_rfc2822_alternative(
    from: &str,
    to: &[String],
    subject: &str,
    plain_text: &str,
    html: &str,
    attachments: &[CommAttachment],
) -> String {
    let alt_boundary = format!("hivemind_alt_{}", uuid_v4_simple());

    let alt_part = format!(
        "--{alt_boundary}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {plain_text}\r\n\
         --{alt_boundary}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         {html}\r\n\
         --{alt_boundary}--\r\n"
    );

    if attachments.is_empty() {
        return format!(
            "From: {from}\r\n\
             To: {to}\r\n\
             Subject: {subject}\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/alternative; boundary=\"{alt_boundary}\"\r\n\
             \r\n\
             {alt_part}",
            to = to.join(", "),
        );
    }

    let mixed_boundary = format!("hivemind_mixed_{}", uuid_v4_simple());
    let mut msg = format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{mixed_boundary}\"\r\n\
         \r\n\
         --{mixed_boundary}\r\n\
         Content-Type: multipart/alternative; boundary=\"{alt_boundary}\"\r\n\
         \r\n\
         {alt_part}\r\n",
        to = to.join(", "),
    );

    for att in attachments {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&att.data);
        msg.push_str(&format!(
            "--{mixed_boundary}\r\n\
             Content-Type: {media_type}; name=\"{filename}\"\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             {encoded}\r\n",
            media_type = att.media_type,
            filename = att.filename,
        ));
    }

    msg.push_str(&format!("--{mixed_boundary}--\r\n"));
    msg
}

/// Cheap pseudo-UUID for MIME boundaries (no crypto requirement).
fn uuid_v4_simple() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{nanos:032x}")
}

// ---------------------------------------------------------------------------
// Gmail API response helpers
// ---------------------------------------------------------------------------

/// Find a header value from the Gmail `payload.headers` array.
fn header_value(headers: &[serde_json::Value], name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    headers
        .iter()
        .find(|h| h["name"].as_str().map(|n| n.to_lowercase() == lower).unwrap_or(false))
        .and_then(|h| h["value"].as_str())
        .map(|s| s.to_string())
}

/// Recursively extract body text from a Gmail `payload` object.
///
/// Gmail returns `payload.body.data` for single-part messages, or nests the
/// body inside `payload.parts[]` for multipart.  We prefer `text/plain`.
fn extract_body(payload: &serde_json::Value) -> String {
    // Single-part body
    if let Some(data) = payload["body"]["data"].as_str() {
        if let Ok(bytes) = b64url().decode(data) {
            if let Ok(text) = String::from_utf8(bytes) {
                return text;
            }
        }
    }

    // Multipart: recurse into parts, preferring text/plain
    if let Some(parts) = payload["parts"].as_array() {
        // First pass: look for text/plain
        for part in parts {
            let mime = part["mimeType"].as_str().unwrap_or("");
            if mime == "text/plain" {
                if let Some(data) = part["body"]["data"].as_str() {
                    if let Ok(bytes) = b64url().decode(data) {
                        if let Ok(text) = String::from_utf8(bytes) {
                            return text;
                        }
                    }
                }
            }
        }
        // Second pass: recurse into nested multipart containers
        for part in parts {
            let result = extract_body(part);
            if !result.is_empty() {
                return result;
            }
        }
    }

    String::new()
}

/// Extract attachment metadata from a Gmail message payload.
///
/// Gmail parts with a `body.attachmentId` are attachments. We collect their
/// IDs, filenames, and MIME types without downloading the actual content.
fn extract_attachment_metadata(payload: &serde_json::Value) -> Vec<CommAttachment> {
    let mut attachments = Vec::new();
    collect_attachments(payload, &mut attachments);
    attachments
}

fn collect_attachments(part: &serde_json::Value, out: &mut Vec<CommAttachment>) {
    // Check if this part has an attachmentId (indicates it's an attachment)
    if let Some(att_id) = part["body"]["attachmentId"].as_str() {
        let filename =
            part["filename"].as_str().filter(|s| !s.is_empty()).unwrap_or("attachment").to_string();
        let media_type =
            part["mimeType"].as_str().unwrap_or("application/octet-stream").to_string();
        out.push(CommAttachment {
            id: Some(att_id.to_string()),
            filename,
            media_type,
            data: Vec::new(), // metadata only; use download_attachment for content
        });
    }

    // Recurse into sub-parts (multipart containers)
    if let Some(parts) = part["parts"].as_array() {
        for sub in parts {
            collect_attachments(sub, out);
        }
    }
}

// ---------------------------------------------------------------------------
// CommunicationService implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl CommunicationService for GmailCommunication {
    fn name(&self) -> &str {
        "Google Gmail"
    }

    async fn test_connection(&self) -> Result<()> {
        let url = format!("{GMAIL_API}/profile");
        self.google.get(&url).await?;
        info!(connector = %self.connector_id, "Gmail API connection test OK");
        Ok(())
    }

    async fn send(
        &self,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let subject = subject.unwrap_or("");

        let raw_message = if attachments.is_empty() {
            build_rfc2822_simple(&self.from_address, to, subject, body, "text/plain; charset=utf-8")
        } else {
            build_rfc2822_with_attachments(
                &self.from_address,
                to,
                subject,
                body,
                "text/plain; charset=utf-8",
                attachments,
            )
        };

        let encoded = b64url().encode(raw_message.as_bytes());
        let payload = serde_json::json!({ "raw": encoded });

        let url = format!("{GMAIL_API}/messages/send");
        let resp = self
            .google
            .post(&url, &payload)
            .await
            .context("Gmail API messages/send request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("Gmail API messages/send failed ({status}): {err_body}");
        }

        let body: serde_json::Value = resp.json().await.context("parsing send response")?;
        let msg_id = body["id"].as_str().unwrap_or("sent").to_string();

        info!(connector = %self.connector_id, to = ?to, msg_id = %msg_id, "email sent via Gmail API");
        Ok(msg_id)
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

        let subject = subject.unwrap_or("");

        let raw_message = build_rfc2822_alternative(
            &self.from_address,
            to,
            subject,
            fallback_text,
            &html,
            attachments,
        );

        let encoded = b64url().encode(raw_message.as_bytes());
        let payload = serde_json::json!({ "raw": encoded });

        let url = format!("{GMAIL_API}/messages/send");
        let resp = self
            .google
            .post(&url, &payload)
            .await
            .context("Gmail API messages/send (HTML) request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("Gmail API messages/send (HTML) failed ({status}): {err_body}");
        }

        let body: serde_json::Value = resp.json().await.context("parsing send_rich response")?;
        let msg_id = body["id"].as_str().unwrap_or("sent").to_string();

        info!(connector = %self.connector_id, to = ?to, msg_id = %msg_id, "HTML email sent via Gmail API");
        Ok(msg_id)
    }

    async fn fetch_new(&self, limit: usize) -> Result<Vec<InboundMessage>> {
        // List unread messages in the configured label.
        let label = urlencoding::encode(&self.folder);
        let url = format!("{GMAIL_API}/messages?q=is:unread&labelIds={label}&maxResults={limit}",);

        let list_body = self.google.get(&url).await?;
        let items = list_body["messages"].as_array().cloned().unwrap_or_default();

        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut messages = Vec::new();

        for item in &items {
            let gmail_id = match item["id"].as_str() {
                Some(id) => id,
                None => continue,
            };

            // Fetch the full message.
            let msg_url = format!("{GMAIL_API}/messages/{gmail_id}?format=full");
            let msg = match self.google.get(&msg_url).await {
                Ok(m) => m,
                Err(e) => {
                    debug!(
                        connector = %self.connector_id,
                        gmail_id,
                        error = %e,
                        "failed to fetch message, skipping"
                    );
                    continue;
                }
            };

            let payload = &msg["payload"];
            let headers = payload["headers"].as_array().cloned().unwrap_or_default();

            let from = header_value(&headers, "From").unwrap_or_else(|| "unknown".to_string());
            let to_header = header_value(&headers, "To").unwrap_or_default();
            let to_addrs: Vec<String> = to_header
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let subject = header_value(&headers, "Subject");
            let body_text = extract_body(payload);

            // Gmail `internalDate` is milliseconds since epoch (as a string).
            let timestamp_ms: u128 =
                msg["internalDate"].as_str().and_then(|s| s.parse::<u128>().ok()).unwrap_or_else(
                    || SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
                );

            let thread_id = msg["threadId"].as_str().unwrap_or_default().to_string();

            let mut metadata = HashMap::new();
            metadata.insert("gmail_message_id".to_string(), gmail_id.to_string());
            metadata.insert("thread_id".to_string(), thread_id);
            metadata.insert("label".to_string(), label.to_string());

            // Extract reply-tracking headers.
            if let Some(v) = header_value(&headers, "Message-ID") {
                metadata.insert("message_id".to_string(), v);
            }
            if let Some(v) = header_value(&headers, "In-Reply-To") {
                metadata.insert("in_reply_to".to_string(), v);
            }
            if let Some(v) = header_value(&headers, "References") {
                metadata.insert("references".to_string(), v);
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

            // Extract attachment metadata from MIME parts.
            let attachments = extract_attachment_metadata(payload);

            messages.push(InboundMessage {
                external_id: format!("{}:gmail:{}", self.connector_id, gmail_id),
                from,
                to: to_addrs,
                subject,
                body: body_text,
                timestamp_ms,
                metadata,
                attachments,
            });
        }

        debug!(
            connector = %self.connector_id,
            count = messages.len(),
            "fetched emails via Gmail API"
        );
        Ok(messages)
    }

    async fn mark_seen(&self, message_id: &str) -> Result<()> {
        // external_id format: "{connector_id}:gmail:{gmail_msg_id}"
        let gmail_id = message_id.rsplit_once(":gmail:").map(|(_, id)| id).unwrap_or(message_id);

        let url = format!("{GMAIL_API}/messages/{gmail_id}/modify");
        let payload = serde_json::json!({
            "removeLabelIds": ["UNREAD"]
        });

        let resp =
            self.google.post(&url, &payload).await.context("Gmail API mark_seen request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            bail!("Gmail API mark_seen failed ({status}): {err_body}");
        }

        debug!(
            connector = %self.connector_id,
            message_id,
            "marked message as read via Gmail API"
        );
        Ok(())
    }

    fn supports_idle(&self) -> bool {
        false
    }

    async fn list_channels(&self) -> Result<Vec<ChannelInfo>> {
        let url = format!("{GMAIL_API}/labels");
        let body = self.google.get(&url).await?;
        let labels = body["labels"].as_array().cloned().unwrap_or_default();

        let channels = labels
            .iter()
            .filter_map(|label| {
                let id = label["id"].as_str()?.to_string();
                let name = label["name"].as_str()?.to_string();
                let label_type = label["type"].as_str().map(|s| s.to_string());
                Some(ChannelInfo { id, name, channel_type: label_type, group_name: None })
            })
            .collect();

        Ok(channels)
    }

    async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<CommAttachment> {
        // message_id may be a full external_id (connector_id:gmail:gmail_msg_id) or raw gmail id
        let gmail_msg_id =
            message_id.rsplit_once(":gmail:").map(|(_, id)| id).unwrap_or(message_id);

        let url = format!("{GMAIL_API}/messages/{gmail_msg_id}/attachments/{attachment_id}",);

        let body = self.google.get(&url).await.context("Gmail API download attachment failed")?;

        // Gmail returns attachment data as URL-safe base64 (RFC 4648 §5)
        let data_b64url = body["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("attachment response missing data field"))?;

        let data = b64url().decode(data_b64url).context("failed to decode attachment data")?;

        // We don't get the filename from the attachment endpoint itself,
        // so return a generic name. The caller should use metadata from fetch_new.
        Ok(CommAttachment {
            id: Some(attachment_id.to_string()),
            filename: "attachment".to_string(),
            media_type: "application/octet-stream".to_string(),
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_attachment_metadata_single_attachment() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {
                    "mimeType": "text/plain",
                    "body": { "data": "aGVsbG8=" }
                },
                {
                    "mimeType": "application/pdf",
                    "filename": "report.pdf",
                    "body": { "attachmentId": "ATT_001", "size": 12345 }
                }
            ]
        });

        let attachments = extract_attachment_metadata(&payload);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id.as_deref(), Some("ATT_001"));
        assert_eq!(attachments[0].filename, "report.pdf");
        assert_eq!(attachments[0].media_type, "application/pdf");
        assert!(attachments[0].data.is_empty(), "data should be empty (metadata only)");
    }

    #[test]
    fn test_extract_attachment_metadata_multiple_attachments() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {
                    "mimeType": "text/plain",
                    "body": { "data": "aGVsbG8=" }
                },
                {
                    "mimeType": "image/png",
                    "filename": "photo.png",
                    "body": { "attachmentId": "ATT_A", "size": 100 }
                },
                {
                    "mimeType": "application/zip",
                    "filename": "archive.zip",
                    "body": { "attachmentId": "ATT_B", "size": 200 }
                }
            ]
        });

        let attachments = extract_attachment_metadata(&payload);
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].filename, "photo.png");
        assert_eq!(attachments[1].filename, "archive.zip");
    }

    #[test]
    fn test_extract_attachment_metadata_nested_multipart() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {
                    "mimeType": "multipart/alternative",
                    "parts": [
                        { "mimeType": "text/plain", "body": { "data": "aGVsbG8=" } },
                        { "mimeType": "text/html", "body": { "data": "PGI+aGk8L2I+" } }
                    ]
                },
                {
                    "mimeType": "application/pdf",
                    "filename": "deep.pdf",
                    "body": { "attachmentId": "ATT_DEEP", "size": 999 }
                }
            ]
        });

        let attachments = extract_attachment_metadata(&payload);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id.as_deref(), Some("ATT_DEEP"));
        assert_eq!(attachments[0].filename, "deep.pdf");
    }

    #[test]
    fn test_extract_attachment_metadata_no_attachments() {
        let payload = json!({
            "mimeType": "text/plain",
            "body": { "data": "aGVsbG8=" }
        });

        let attachments = extract_attachment_metadata(&payload);
        assert!(attachments.is_empty());
    }

    #[test]
    fn test_extract_attachment_metadata_missing_filename_defaults() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {
                    "mimeType": "application/octet-stream",
                    "filename": "",
                    "body": { "attachmentId": "ATT_NONAME", "size": 10 }
                }
            ]
        });

        let attachments = extract_attachment_metadata(&payload);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "attachment");
    }
}
