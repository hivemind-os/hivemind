use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::config::SmtpEncryption;
use crate::message_state::MessageStateStore;
use crate::services::communication::{
    CommAttachment, CommunicationService, InboundMessage, RichMessageBody,
};
use crate::MessageState;

/// IMAP/SMTP communication service using password-based authentication.
///
/// Ported from the `EmailConnector` in `hive-comms`. Uses IMAP for reading
/// mail and `lettre` SMTP for sending. Message state (UID validity, seen UIDs)
/// is tracked via the shared [`MessageState`] SQLite store.
pub struct ImapCommunication {
    connector_id: String,
    /// IMAP server settings
    imap_host: String,
    imap_port: u16,
    /// SMTP server settings
    smtp_host: String,
    smtp_port: u16,
    smtp_encryption: SmtpEncryption,
    /// Auth credentials
    username: String,
    password: String,
    /// Sender address for outbound mail
    from_address: String,
    /// IMAP folder to monitor (e.g. "INBOX")
    folder: String,
    /// Persistent message tracking state
    state: Arc<Mutex<MessageState>>,
}

impl ImapCommunication {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connector_id: &str,
        imap_host: String,
        imap_port: u16,
        smtp_host: String,
        smtp_port: u16,
        smtp_encryption: SmtpEncryption,
        username: String,
        password: String,
        from_address: String,
        folder: String,
        state: Arc<Mutex<MessageState>>,
    ) -> Self {
        Self {
            connector_id: connector_id.to_string(),
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            smtp_encryption,
            username,
            password,
            from_address,
            folder,
            state,
        }
    }
}

use crate::mail_utils::parse_rfc822_body;

/// Send email via SMTP with password credentials (blocking).
#[allow(clippy::too_many_arguments)]
fn send_smtp_inner(
    smtp_host: &str,
    smtp_port: u16,
    smtp_encryption: SmtpEncryption,
    from_address: &str,
    username: &str,
    password: &str,
    connector_id: &str,
    to: &[String],
    subject: Option<&str>,
    body: &str,
    attachments: &[CommAttachment],
    html_body: Option<&str>,
) -> Result<String> {
    use lettre::message::header::ContentType;
    use lettre::message::{Attachment, MultiPart, SinglePart};
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let mut builder = Message::builder().from(
        from_address.parse().with_context(|| format!("invalid from address: {from_address}"))?,
    );

    for addr in to {
        builder =
            builder.to(addr.parse().with_context(|| format!("invalid to address: {addr}"))?);
    }

    if let Some(subj) = subject {
        builder = builder.subject(subj);
    }

    let message = match (html_body, attachments.is_empty()) {
        // HTML body, no attachments
        (Some(html), true) => {
            let html_part =
                SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string());
            let text_part =
                SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body.to_string());
            builder
                .multipart(MultiPart::alternative().singlepart(text_part).singlepart(html_part))
                .context("building alternative email")?
        }
        // HTML body with attachments
        (Some(html), false) => {
            let html_part =
                SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string());
            let text_part =
                SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body.to_string());
            let alt = MultiPart::alternative().singlepart(text_part).singlepart(html_part);
            let mut mixed = MultiPart::mixed().multipart(alt);
            for att in attachments {
                let content_type =
                    ContentType::parse(&att.media_type).unwrap_or(ContentType::TEXT_PLAIN);
                let attachment =
                    Attachment::new(att.filename.clone()).body(att.data.clone(), content_type);
                mixed = mixed.singlepart(attachment);
            }
            builder.multipart(mixed).context("building multipart email with HTML")?
        }
        // Plain text, no attachments
        (None, true) => builder
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .context("building email message")?,
        // Plain text with attachments
        (None, false) => {
            let text_part =
                SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body.to_string());
            let mut multipart = MultiPart::mixed().singlepart(text_part);
            for att in attachments {
                let content_type =
                    ContentType::parse(&att.media_type).unwrap_or(ContentType::TEXT_PLAIN);
                let attachment =
                    Attachment::new(att.filename.clone()).body(att.data.clone(), content_type);
                multipart = multipart.singlepart(attachment);
            }
            builder.multipart(multipart).context("building multipart email")?
        }
    };

    let creds = Credentials::new(username.to_string(), password.to_string());
    let transport = match smtp_encryption {
        SmtpEncryption::Starttls => SmtpTransport::starttls_relay(smtp_host)
            .with_context(|| format!("building SMTP STARTTLS relay for {smtp_host}"))?
            .port(smtp_port)
            .credentials(creds)
            .build(),
        SmtpEncryption::ImplicitTls => SmtpTransport::relay(smtp_host)
            .with_context(|| format!("building SMTP implicit-TLS relay for {smtp_host}"))?
            .port(smtp_port)
            .credentials(creds)
            .build(),
    };

    let response = transport.send(&message).context("sending email via SMTP")?;
    let message_id =
        response.message().next().map(|l| l.to_string()).unwrap_or_else(|| "sent".to_string());

    info!(connector = %connector_id, to = ?to, "email sent successfully");
    Ok(message_id)
}

#[async_trait]
impl CommunicationService for ImapCommunication {
    fn name(&self) -> &str {
        "IMAP"
    }

    async fn test_connection(&self) -> Result<()> {
        let imap_host = self.imap_host.clone();
        let imap_port = self.imap_port;
        let smtp_host = self.smtp_host.clone();
        let smtp_port = self.smtp_port;
        let smtp_encryption = self.smtp_encryption;
        let username = self.username.clone();
        let password = self.password.clone();
        let folder = self.folder.clone();
        let connector_id = self.connector_id.clone();

        crate::spawn_blocking_with_span(move || {
            // --- Test IMAP ---
            {
                let tls = native_tls::TlsConnector::builder()
                    .build()
                    .context("building TLS connector")?;

                let client = imap::connect((imap_host.as_str(), imap_port), &imap_host, &tls)
                    .with_context(|| format!("IMAP: connecting to {imap_host}"))?;

                let mut session = client
                    .login(&username, &password)
                    .map_err(|e| anyhow::anyhow!("IMAP login failed: {}", e.0))?;

                session
                    .select(&folder)
                    .with_context(|| format!("IMAP: selecting folder {folder}"))?;

                let _ = session.logout();
                info!(connector = %connector_id, "IMAP test OK");
            }

            // --- Test SMTP ---
            {
                use lettre::transport::smtp::authentication::Credentials;
                use lettre::SmtpTransport;

                let creds = Credentials::new(username, password);
                let transport = match smtp_encryption {
                    SmtpEncryption::Starttls => SmtpTransport::starttls_relay(&smtp_host)
                        .with_context(|| format!("SMTP: connecting to {smtp_host} (STARTTLS)"))?
                        .port(smtp_port)
                        .credentials(creds)
                        .build(),
                    SmtpEncryption::ImplicitTls => SmtpTransport::relay(&smtp_host)
                        .with_context(|| format!("SMTP: connecting to {smtp_host} (implicit TLS)"))?
                        .port(smtp_port)
                        .credentials(creds)
                        .build(),
                };

                transport.test_connection().with_context(|| {
                    format!(
                        "SMTP authentication failed ({smtp_encryption:?} on port {smtp_port}). \
                         If using Amazon WorkMail, set encryption to 'implicit-tls' and port to 465. \
                         For Outlook/Office365: ensure 'Authenticated SMTP' is enabled \
                         for the mailbox (Set-CASMailbox -SmtpClientAuthenticationDisabled $false)"
                    )
                })?;
                info!(connector = %connector_id, "SMTP test OK");
            }

            info!(connector = %connector_id, "email connection test successful (IMAP + SMTP)");
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("spawn_blocking for test_connection")?
    }

    async fn send(
        &self,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let smtp_host = self.smtp_host.clone();
        let smtp_port = self.smtp_port;
        let smtp_encryption = self.smtp_encryption;
        let from_address = self.from_address.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let connector_id = self.connector_id.clone();
        let to_owned: Vec<String> = to.to_vec();
        let subject_owned = subject.map(|s| s.to_string());
        let body_owned = body.to_string();
        let attachments_owned: Vec<CommAttachment> = attachments.to_vec();

        crate::spawn_blocking_with_span(move || {
            send_smtp_inner(
                &smtp_host,
                smtp_port,
                smtp_encryption,
                &from_address,
                &username,
                &password,
                &connector_id,
                &to_owned,
                subject_owned.as_deref(),
                &body_owned,
                &attachments_owned,
                None,
            )
        })
        .await
        .context("spawn_blocking for send")?
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
            Some(RichMessageBody::Html(h)) => Some(h.clone()),
            _ => None,
        };

        if html.is_none() {
            return self.send(to, subject, fallback_text, attachments).await;
        }

        let smtp_host = self.smtp_host.clone();
        let smtp_port = self.smtp_port;
        let smtp_encryption = self.smtp_encryption;
        let from_address = self.from_address.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let connector_id = self.connector_id.clone();
        let to_owned: Vec<String> = to.to_vec();
        let subject_owned = subject.map(|s| s.to_string());
        let body_owned = fallback_text.to_string();
        let attachments_owned: Vec<CommAttachment> = attachments.to_vec();

        crate::spawn_blocking_with_span(move || {
            send_smtp_inner(
                &smtp_host,
                smtp_port,
                smtp_encryption,
                &from_address,
                &username,
                &password,
                &connector_id,
                &to_owned,
                subject_owned.as_deref(),
                &body_owned,
                &attachments_owned,
                html.as_deref(),
            )
        })
        .await
        .context("spawn_blocking for send_rich")?
    }

    async fn fetch_new(&self, limit: usize) -> Result<Vec<InboundMessage>> {
        let imap_host = self.imap_host.clone();
        let imap_port = self.imap_port;
        let username = self.username.clone();
        let password = self.password.clone();
        let folder = self.folder.clone();
        let state = self.state.clone();
        let connector_id = self.connector_id.clone();

        crate::spawn_blocking_with_span(move || {
            let tls =
                native_tls::TlsConnector::builder().build().context("building TLS connector")?;

            let client = imap::connect((imap_host.as_str(), imap_port), &imap_host, &tls)
                .context("IMAP connect for fetch")?;

            let mut session = client
                .login(&username, &password)
                .map_err(|e| anyhow::anyhow!("IMAP login failed: {}", e.0))?;

            let mailbox = session.select(&folder).with_context(|| format!("selecting {folder}"))?;

            let server_validity = mailbox.uid_validity.unwrap_or(0);

            // Check UID validity
            {
                let msg_state = state.lock();
                let reset = msg_state
                    .update_uid_validity(&folder, server_validity)
                    .context("updating uid validity")?;
                if reset {
                    warn!(connector = %connector_id, "IMAP UID validity changed, full re-sync");
                }
            }

            // Determine search range
            let last_uid = {
                let msg_state = state.lock();
                msg_state.last_seen_uid(&folder).unwrap_or(0)
            };

            // Use UID range when we have state; otherwise get all messages
            // and take the most recent ones.  We never rely on the IMAP \Seen
            // flag — our local state DB tracks what has been processed.
            let search_query = if last_uid > 0 {
                format!("UID {}:*", last_uid + 1)
            } else {
                "ALL".to_string()
            };

            debug!(
                connector = %connector_id,
                last_uid,
                search_query = %search_query,
                "IMAP fetch_new: searching for messages"
            );

            let uids = session
                .uid_search(&search_query)
                .with_context(|| format!("IMAP UID SEARCH {search_query}"))?;

            debug!(
                connector = %connector_id,
                uid_count = uids.len(),
                "IMAP fetch_new: search returned UIDs"
            );

            if uids.is_empty() {
                let _ = session.logout();
                return Ok(Vec::new());
            }

            // Filter out already-seen UIDs and apply limit.
            // Also filter out the last_uid itself when using UID range search,
            // since IMAP `UID n:*` is inclusive and may return n.
            // Sort descending so we process the most recent messages first.
            let unseen_uids: Vec<u32> = {
                let msg_state = state.lock();
                let mut candidates: Vec<u32> = uids.iter()
                    .copied()
                    .filter(|uid| *uid > last_uid)
                    .filter(|uid| {
                        match msg_state.is_seen(&folder, *uid) {
                            Ok(seen) => !seen,
                            Err(e) => {
                                warn!(uid, error = %e, "state DB error checking seen status, including message");
                                true // include on error rather than skip
                            }
                        }
                    })
                    .collect();
                candidates.sort_unstable_by(|a, b| b.cmp(a)); // newest first
                candidates.truncate(limit);
                candidates.sort_unstable(); // back to ascending for fetch
                candidates
            };

            debug!(
                connector = %connector_id,
                unseen_count = unseen_uids.len(),
                "IMAP fetch_new: filtered to unseen UIDs"
            );

            if unseen_uids.is_empty() {
                let _ = session.logout();
                return Ok(Vec::new());
            }

            // Fetch the messages
            let uid_set = unseen_uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");

            let fetches = session
                .uid_fetch(&uid_set, "(BODY.PEEK[] ENVELOPE INTERNALDATE)")
                .with_context(|| format!("fetching UIDs {uid_set}"))?;

            let mut messages = Vec::new();

            for fetch in fetches.iter() {
                let uid = match fetch.uid {
                    Some(u) => u,
                    None => continue,
                };

                // Parse the RFC822 body with mail-parser for proper MIME handling
                let raw_body = fetch.body().unwrap_or_default();
                let (body_text, parsed_attachments) = parse_rfc822_body(raw_body);

                let envelope = fetch.envelope();

                let from = envelope
                    .as_ref()
                    .and_then(|env| env.from.as_ref())
                    .and_then(|addrs| addrs.first())
                    .map(|addr| {
                        let mailbox = addr
                            .mailbox
                            .as_ref()
                            .map(|m| String::from_utf8_lossy(m).to_string())
                            .unwrap_or_default();
                        let host = addr
                            .host
                            .as_ref()
                            .map(|h| String::from_utf8_lossy(h).to_string())
                            .unwrap_or_default();
                        format!("{mailbox}@{host}")
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                let subject = envelope
                    .as_ref()
                    .and_then(|env| env.subject.as_ref())
                    .map(|s| String::from_utf8_lossy(s).to_string());

                let to_addrs = envelope
                    .as_ref()
                    .and_then(|env| env.to.as_ref())
                    .map(|addrs| {
                        addrs
                            .iter()
                            .map(|addr| {
                                let mailbox = addr
                                    .mailbox
                                    .as_ref()
                                    .map(|m| String::from_utf8_lossy(m).to_string())
                                    .unwrap_or_default();
                                let host = addr
                                    .host
                                    .as_ref()
                                    .map(|h| String::from_utf8_lossy(h).to_string())
                                    .unwrap_or_default();
                                format!("{mailbox}@{host}")
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let timestamp_ms =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();

                let mut metadata = HashMap::new();
                metadata.insert("imap_uid".to_string(), uid.to_string());
                metadata.insert("folder".to_string(), folder.clone());

                // Extract reply-tracking headers from the IMAP envelope.
                if let Some(env) = envelope {
                    if let Some(in_reply_to) = env.in_reply_to {
                        metadata.insert(
                            "in_reply_to".to_string(),
                            String::from_utf8_lossy(in_reply_to).to_string(),
                        );
                    }
                    if let Some(msg_id) = env.message_id {
                        metadata.insert(
                            "message_id".to_string(),
                            String::from_utf8_lossy(msg_id).to_string(),
                        );
                    }
                }

                // Also try to extract [HIVEMIND:<token>] from subject for reply matching.
                if let Some(ref subj) = subject {
                    if let Some(start) = subj.find("[HIVEMIND:") {
                        if let Some(end) = subj[start..].find(']') {
                            let token = &subj[start + 7..start + end];
                            metadata.insert("hivemind_token".to_string(), token.to_string());
                        }
                    }
                }

                messages.push(InboundMessage {
                    external_id: format!("{connector_id}:{folder}:{uid}"),
                    from,
                    to: to_addrs,
                    subject,
                    body: body_text,
                    timestamp_ms,
                    metadata,
                    attachments: parsed_attachments,
                });

                // Mark as seen in our state DB
                let msg_state = state.lock();
                if let Err(e) = msg_state.mark_seen(&folder, uid) {
                    warn!(error = %e, uid, "failed to mark UID as seen");
                }
            }

            debug!(
                connector = %connector_id,
                count = messages.len(),
                "fetched new emails"
            );

            let _ = session.logout();
            Ok(messages)
        })
        .await
        .context("spawn_blocking for fetch_new")?
    }

    async fn mark_seen(&self, message_id: &str) -> Result<()> {
        // message_id format: "connector_id:folder:uid"
        let parts: Vec<&str> = message_id.splitn(3, ':').collect();
        if parts.len() < 3 {
            bail!("invalid message_id format: {message_id}");
        }

        let folder = parts[1].to_string();
        let uid: u32 =
            parts[2].parse().with_context(|| format!("invalid UID in message_id: {message_id}"))?;

        let state = self.state.clone();

        crate::spawn_blocking_with_span(move || {
            let msg_state = state.lock();
            msg_state.mark_seen(&folder, uid)
        })
        .await
        .context("spawn_blocking for mark_seen")?
    }

    fn supports_idle(&self) -> bool {
        true
    }

    async fn wait_for_changes(&self, timeout: Duration) -> Result<bool> {
        let imap_host = self.imap_host.clone();
        let imap_port = self.imap_port;
        let username = self.username.clone();
        let password = self.password.clone();
        let folder = self.folder.clone();
        let connector_id = self.connector_id.clone();
        // Cap the blocking IDLE at 30s so the thread returns promptly on
        // shutdown.  The poll loop will re-enter IDLE if still running.
        let capped = timeout.min(Duration::from_secs(30));

        crate::spawn_blocking_with_span(move || {
            let tls = native_tls::TlsConnector::builder()
                .build()
                .context("building TLS connector for IDLE")?;

            let client = imap::connect((imap_host.as_str(), imap_port), &imap_host, &tls)
                .context("IMAP connect for IDLE")?;

            let mut session = client
                .login(&username, &password)
                .map_err(|e| anyhow::anyhow!("IMAP login failed: {}", e.0))?;

            session.select(&folder).with_context(|| format!("selecting {folder} for IDLE"))?;

            debug!(connector = %connector_id, "entering IMAP IDLE");

            let idle_handle = session.idle().context("starting IDLE")?;
            let result = idle_handle.wait_with_timeout(capped);

            let has_new = match result {
                Ok(reason) => {
                    debug!(connector = %connector_id, ?reason, "IDLE returned");
                    !matches!(reason, imap::extensions::idle::WaitOutcome::TimedOut)
                }
                Err(e) => {
                    warn!(connector = %connector_id, error = %e, "IDLE failed, falling back");
                    false
                }
            };

            let _ = session.logout();
            Ok(has_new)
        })
        .await
        .context("spawn_blocking for IDLE")?
    }
}
