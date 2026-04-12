//! IWA (iWork Archive) file parser.
//!
//! IWA files are Snappy-framed streams containing a sequence of protobuf
//! archives. Each archive has an `ArchiveInfo` header followed by one or more
//! message payloads.
//!
//! The framing format:
//! - The stream is divided into Snappy chunks, each with a 4-byte header:
//!   [type (1 byte)] [length (3 bytes, little-endian)]
//!   Type 0 = Snappy-compressed data.
//! - After decompression, the stream contains consecutive archives:
//!   [varint archiveInfoLength] [ArchiveInfo protobuf] [payload bytes...]
//!
//! ArchiveInfo proto:
//!   field 1 (varint): identifier (object ID, unique across the document)
//!   field 2 (embedded): repeated MessageInfo
//!
//! MessageInfo proto:
//!   field 1 (varint): type (message type number)
//!   field 3 (varint): length (payload byte count)

use crate::proto;

/// A decoded archive from an IWA stream.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Archive {
    /// Object identifier (unique across the document).
    pub identifier: u64,
    /// The messages contained in this archive.
    pub messages: Vec<Message>,
}

/// A single message payload within an archive.
#[derive(Debug)]
pub struct Message {
    /// Message type number (maps to a protobuf message type per app).
    pub message_type: u32,
    /// Raw protobuf-encoded payload bytes.
    pub payload: Vec<u8>,
}

/// Decompress a Snappy-framed IWA stream.
///
/// IWA files use a simplified Snappy framing: each chunk has a 1-byte type
/// (always 0) and a 3-byte little-endian length. No stream identifier or
/// CRC checksums are included.
fn decompress_iwa(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        if pos + 4 > data.len() {
            break;
        }

        let chunk_type = data[pos];
        let chunk_len =
            data[pos + 1] as usize | (data[pos + 2] as usize) << 8 | (data[pos + 3] as usize) << 16;
        pos += 4;

        if pos + chunk_len > data.len() {
            anyhow::bail!("IWA chunk extends past end of data");
        }

        let chunk_data = &data[pos..pos + chunk_len];
        pos += chunk_len;

        if chunk_type != 0 {
            // Only type 0 (compressed) chunks are expected in iWork files.
            continue;
        }

        let decompressed = snap::raw::Decoder::new()
            .decompress_vec(chunk_data)
            .map_err(|e| anyhow::anyhow!("Snappy decompression failed: {e}"))?;
        output.extend_from_slice(&decompressed);
    }

    Ok(output)
}

/// Parse all archives from a decompressed IWA byte stream.
fn parse_archives(data: &[u8]) -> Vec<Archive> {
    let mut archives = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Read the ArchiveInfo length (varint)
        let (info_len, n) = match proto::decode_varint(&data[pos..]) {
            Some(v) => v,
            None => break,
        };
        pos += n;

        let info_len = info_len as usize;
        if pos + info_len > data.len() {
            break;
        }

        // Decode the ArchiveInfo fields
        let info_bytes = &data[pos..pos + info_len];
        pos += info_len;

        let info_fields = proto::decode_fields(info_bytes);

        let identifier = proto::get_varint(&info_fields, 1).unwrap_or(0);

        // Parse each MessageInfo (field 2, repeated embedded message)
        let message_infos_raw = proto::get_repeated_bytes(&info_fields, 2);

        let mut messages = Vec::new();
        for mi_bytes in message_infos_raw {
            let mi_fields = proto::decode_fields(mi_bytes);
            let message_type = proto::get_varint(&mi_fields, 1).unwrap_or(0) as u32;
            let length = proto::get_varint(&mi_fields, 3).unwrap_or(0) as usize;

            if pos + length > data.len() {
                break;
            }

            let payload = data[pos..pos + length].to_vec();
            pos += length;

            messages.push(Message { message_type, payload });
        }

        archives.push(Archive { identifier, messages });
    }

    archives
}

/// Parse all archives from raw (Snappy-compressed) IWA data.
pub fn parse_iwa(data: &[u8]) -> anyhow::Result<Vec<Archive>> {
    let decompressed = decompress_iwa(data)?;
    Ok(parse_archives(&decompressed))
}

/// TSWP.StorageArchive message types (shared across Pages, Numbers, Keynote).
const TSWP_STORAGE_ARCHIVE_TYPES: &[u32] = &[2001, 2005];

/// Extract all text strings from TSWP.StorageArchive messages in the given
/// archives.
///
/// TSWP.StorageArchive field 3 = `repeated string text`.
pub fn extract_text_from_archives(archives: &[Archive]) -> Vec<String> {
    let mut texts = Vec::new();

    for archive in archives {
        for message in &archive.messages {
            if !TSWP_STORAGE_ARCHIVE_TYPES.contains(&message.message_type) {
                continue;
            }

            let fields = proto::decode_fields(&message.payload);
            let text_fields = proto::get_repeated_bytes(&fields, 3);

            for text_bytes in text_fields {
                if let Ok(s) = std::str::from_utf8(text_bytes) {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        texts.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_iwa_empty() {
        let result = decompress_iwa(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn parse_archives_empty() {
        let archives = parse_archives(&[]);
        assert!(archives.is_empty());
    }

    #[test]
    fn extract_text_from_empty_archives() {
        let texts = extract_text_from_archives(&[]);
        assert!(texts.is_empty());
    }

    #[test]
    fn extract_text_skips_non_tswp_types() {
        let archive = Archive {
            identifier: 1,
            messages: vec![Message {
                message_type: 200, // TSK.DocumentArchive, not TSWP
                payload: vec![],
            }],
        };
        let texts = extract_text_from_archives(&[archive]);
        assert!(texts.is_empty());
    }

    #[test]
    fn extract_text_from_tswp_storage() {
        // Build a minimal TSWP.StorageArchive payload:
        // field 1 (varint) = kind = 0 (BODY): tag = 0x08, value = 0x00
        // field 3 (string) = "Hello World": tag = 0x1a, len = 11
        let text = b"Hello World";
        let mut payload = vec![0x08, 0x00]; // field 1 = 0
        payload.push(0x1a); // field 3 tag
        payload.push(text.len() as u8); // length
        payload.extend_from_slice(text);

        let archive = Archive {
            identifier: 42,
            messages: vec![Message {
                message_type: 2001, // TSWP.StorageArchive
                payload,
            }],
        };

        let texts = extract_text_from_archives(&[archive]);
        assert_eq!(texts, vec!["Hello World"]);
    }
}
