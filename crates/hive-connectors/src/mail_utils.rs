//! Shared utilities for IMAP-based mail connectors (IMAP, Gmail).

use crate::services::communication::CommAttachment;

/// Parse an RFC 822 message body into clean text and attachments.
///
/// Uses `mail-parser` to extract the best text/plain part (falling back to
/// stripped HTML) and any non-inline attachments.  If parsing fails entirely
/// the raw bytes are returned as lossy UTF-8 with no attachments.
pub fn parse_rfc822_body(raw: &[u8]) -> (String, Vec<CommAttachment>) {
    use mail_parser::MimeHeaders;

    let Some(message) = mail_parser::MessageParser::default().parse(raw) else {
        return (String::from_utf8_lossy(raw).to_string(), Vec::new());
    };

    // Best text representation: text/plain → stripped HTML → empty
    let body_text = message
        .body_text(0)
        .map(|t| t.to_string())
        .or_else(|| {
            message.body_html(0).map(|html| {
                let mut out = String::with_capacity(html.len());
                let mut in_tag = false;
                for ch in html.chars() {
                    match ch {
                        '<' => in_tag = true,
                        '>' => in_tag = false,
                        _ if !in_tag => out.push(ch),
                        _ => {}
                    }
                }
                out
            })
        })
        .unwrap_or_default();

    // Non-inline attachments
    let mut attachments = Vec::new();
    for part in message.attachments() {
        let filename = part.attachment_name().unwrap_or("attachment").to_string();
        let media_type = part
            .content_type()
            .map(|ct| {
                let main = ct.ctype();
                match ct.subtype() {
                    Some(sub) => format!("{main}/{sub}"),
                    None => main.to_string(),
                }
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        attachments.push(CommAttachment {
            id: None,
            filename,
            media_type,
            data: part.contents().to_vec(),
        });
    }

    (body_text, attachments)
}
