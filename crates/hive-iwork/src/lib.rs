//! Native parser for Apple iWork files (.pages, .numbers, .key).
//!
//! iWork documents are ZIP archives containing an `Index.zip` (or `Index/`
//! directory) with `.iwa` (iWork Archive) files. Each IWA file is a
//! Snappy-compressed stream of protobuf-encoded objects.
//!
//! This crate extracts text content by parsing the IWA framing and decoding
//! the `TSWP.StorageArchive` message type (types 2001 and 2005), which
//! stores document text across all three iWork applications.
//!
//! # Supported formats
//! - **Pages** (.pages) — word processing documents
//! - **Numbers** (.numbers) — spreadsheets (text cells only)
//! - **Keynote** (.key) — presentations

mod iwa;
mod proto;

use std::io::Read;
use std::path::Path;

/// Extract text from an Apple iWork file (.pages, .numbers, or .key).
///
/// Returns `Ok(Some(text))` if text was successfully extracted,
/// `Ok(None)` if the file contains no extractable text, or `Err` on
/// I/O or parse failures.
///
/// This function opens the ZIP archive, locates all `.iwa` files inside
/// the `Index/` directory (or `Index.zip`), parses their IWA framing, and
/// extracts text from `TSWP.StorageArchive` messages.
pub fn extract_text(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;

    let all_archives = load_archives_from_zip(&mut zip)?;

    if all_archives.is_empty() {
        return Ok(None);
    }

    let texts = iwa::extract_text_from_archives(&all_archives);

    if texts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(texts.join("\n")))
    }
}

/// Load all IWA archives from either an `Index.zip` nested inside the
/// outer ZIP, or directly from `Index/*.iwa` entries.
fn load_archives_from_zip<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> anyhow::Result<Vec<iwa::Archive>> {
    // First, try to find an embedded Index.zip
    if let Some(archives) = try_load_from_index_zip(zip)? {
        return Ok(archives);
    }

    // Otherwise, look for Index/*.iwa files directly in the outer ZIP
    load_from_index_directory(zip)
}

/// Try to load archives from a nested `Index.zip` within the outer archive.
fn try_load_from_index_zip<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> anyhow::Result<Option<Vec<iwa::Archive>>> {
    let mut index_zip_data = Vec::new();
    match zip.by_name("Index.zip") {
        Ok(mut entry) => {
            entry.read_to_end(&mut index_zip_data)?;
        }
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let cursor = std::io::Cursor::new(index_zip_data);
    let mut inner_zip = zip::ZipArchive::new(cursor)?;

    let iwa_names: Vec<String> = (0..inner_zip.len())
        .filter_map(|i| {
            let entry = inner_zip.by_index(i).ok()?;
            let name = entry.name().to_string();
            if name.ends_with(".iwa") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut all_archives = Vec::new();
    for name in iwa_names {
        match parse_iwa_entry(&mut inner_zip, &name) {
            Ok(archives) => all_archives.extend(archives),
            Err(e) => {
                tracing::debug!(file = %name, "Failed to parse IWA entry: {e}");
            }
        }
    }

    Ok(Some(all_archives))
}

/// Load archives from `Index/*.iwa` entries in the outer ZIP.
fn load_from_index_directory<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> anyhow::Result<Vec<iwa::Archive>> {
    let iwa_names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let entry = zip.by_index(i).ok()?;
            let name = entry.name().to_string();
            if name.ends_with(".iwa") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let mut all_archives = Vec::new();
    for name in iwa_names {
        match parse_iwa_entry(zip, &name) {
            Ok(archives) => all_archives.extend(archives),
            Err(e) => {
                tracing::debug!(file = %name, "Failed to parse IWA entry: {e}");
            }
        }
    }

    Ok(all_archives)
}

/// Parse a single `.iwa` entry from a ZIP archive.
fn parse_iwa_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> anyhow::Result<Vec<iwa::Archive>> {
    let mut entry = zip.by_name(name)?;
    let mut data = Vec::new();
    entry.read_to_end(&mut data)?;
    iwa::parse_iwa(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Snappy-compressed IWA stream from raw protobuf data.
    ///
    /// Creates a single Snappy chunk (type 0) wrapping the given data.
    fn make_snappy_iwa(raw: &[u8]) -> Vec<u8> {
        let compressed = snap::raw::Encoder::new().compress_vec(raw).expect("snappy compress");
        let len = compressed.len();
        let mut out = Vec::new();
        out.push(0u8); // chunk type 0
        out.push((len & 0xFF) as u8);
        out.push(((len >> 8) & 0xFF) as u8);
        out.push(((len >> 16) & 0xFF) as u8);
        out.extend_from_slice(&compressed);
        out
    }

    /// Encode a varint into a byte vector.
    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if value == 0 {
                break;
            }
        }
        buf
    }

    /// Build a minimal IWA stream containing a single TSWP.StorageArchive
    /// with the given text.
    fn make_iwa_with_text(text: &str) -> Vec<u8> {
        // Build payload: TSWP.StorageArchive with field 3 = text
        let mut payload = Vec::new();
        payload.push(0x08); // field 1, varint
        payload.push(0x00); // kind = BODY
                            // field 3 (string): tag = (3 << 3) | 2 = 0x1a
        payload.push(0x1a);
        payload.extend_from_slice(&encode_varint(text.len() as u64));
        payload.extend_from_slice(text.as_bytes());

        // Build MessageInfo: type=2001, length=payload.len()
        let mut mi = Vec::new();
        mi.push(0x08); // field 1, varint
        mi.extend_from_slice(&encode_varint(2001));
        mi.push(0x18); // field 3, varint
        mi.extend_from_slice(&encode_varint(payload.len() as u64));

        // Build ArchiveInfo: identifier=1, message_infos=[mi]
        let mut ai = Vec::new();
        ai.push(0x08); // field 1, varint
        ai.push(0x01); // identifier = 1
                       // field 2 (embedded): tag = (2 << 3) | 2 = 0x12
        ai.push(0x12);
        ai.extend_from_slice(&encode_varint(mi.len() as u64));
        ai.extend_from_slice(&mi);

        // Full IWA: varint(ai.len()) + ai + payload
        let mut raw = Vec::new();
        raw.extend_from_slice(&encode_varint(ai.len() as u64));
        raw.extend_from_slice(&ai);
        raw.extend_from_slice(&payload);

        make_snappy_iwa(&raw)
    }

    /// Build an iWork ZIP containing a single IWA file with the given text.
    fn make_iwork_zip(text: &str) -> Vec<u8> {
        let iwa_data = make_iwa_with_text(text);
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("Index/Document.iwa", opts).unwrap();
            z.write_all(&iwa_data).unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_text_from_iwork_zip() {
        let zip_data = make_iwork_zip("Hello from iWork");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pages");
        std::fs::write(&path, &zip_data).unwrap();

        let result = extract_text(&path).unwrap();
        assert!(result.is_some(), "should extract text");
        assert!(result.unwrap().contains("Hello from iWork"));
    }

    #[test]
    fn extract_text_returns_none_for_empty_iwa() {
        // ZIP with an IWA file that has no TSWP.StorageArchive
        let mut payload = Vec::new();
        payload.push(0x08);
        payload.push(0x01);

        let mut mi = Vec::new();
        mi.push(0x08);
        mi.extend_from_slice(&encode_varint(200)); // type 200 = TSK.DocumentArchive
        mi.push(0x18);
        mi.extend_from_slice(&encode_varint(payload.len() as u64));

        let mut ai = Vec::new();
        ai.push(0x08);
        ai.push(0x01);
        ai.push(0x12);
        ai.extend_from_slice(&encode_varint(mi.len() as u64));
        ai.extend_from_slice(&mi);

        let mut raw = Vec::new();
        raw.extend_from_slice(&encode_varint(ai.len() as u64));
        raw.extend_from_slice(&ai);
        raw.extend_from_slice(&payload);

        let iwa_data = make_snappy_iwa(&raw);

        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("Index/Document.iwa", opts).unwrap();
            z.write_all(&iwa_data).unwrap();
            z.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pages");
        std::fs::write(&path, &buf).unwrap();

        let result = extract_text(&path).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn extract_text_multiple_archives() {
        let iwa1 = make_iwa_with_text("First paragraph");
        let iwa2 = make_iwa_with_text("Second paragraph");

        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("Index/Document.iwa", opts).unwrap();
            z.write_all(&iwa1).unwrap();
            z.start_file("Index/Section.iwa", opts).unwrap();
            z.write_all(&iwa2).unwrap();
            z.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.pages");
        std::fs::write(&path, &buf).unwrap();

        let result = extract_text(&path).unwrap().unwrap();
        assert!(result.contains("First paragraph"));
        assert!(result.contains("Second paragraph"));
    }
}
