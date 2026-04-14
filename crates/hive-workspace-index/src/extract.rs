use std::io::Read;
use std::path::Path;

/// Attempt to extract text content from a file.
///
/// Returns `Ok(Some(text))` for supported formats, `Ok(None)` for
/// unsupported/binary files, or `Err` on I/O or parse failures.
pub fn extract_text(path: &Path) -> anyhow::Result<Option<String>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

    match ext.as_str() {
        // Plain text & code
        e if is_text_extension(e) => {
            let content = std::fs::read_to_string(path)?;
            Ok(Some(content))
        }
        "pdf" => extract_pdf(path),
        "docx" => extract_docx(path),
        "pptx" => extract_pptx(path),
        "xlsx" => extract_xlsx(path),
        // Apple iWork formats
        "pages" | "numbers" | "key" => extract_iwork(path),
        // Extensionless files: check filename (e.g. Dockerfile, Makefile)
        _ if is_text_filename(path) => {
            let content = std::fs::read_to_string(path)?;
            Ok(Some(content))
        }
        _ => Ok(None), // unsupported
    }
}

/// Known text/code file extensions.
pub fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "markdown"
            | "rst"
            | "adoc"
            | "rs"
            | "py"
            | "js"
            | "ts"
            | "jsx"
            | "tsx"
            | "java"
            | "kt"
            | "kts"
            | "scala"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "cxx"
            | "cs"
            | "fs"
            | "fsx"
            | "go"
            | "rb"
            | "php"
            | "swift"
            | "m"
            | "mm"
            | "r"
            | "jl"
            | "lua"
            | "pl"
            | "pm"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "bat"
            | "ps1"
            | "psm1"
            | "sql"
            | "graphql"
            | "gql"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "xml"
            | "xsl"
            | "xslt"
            | "svg"
            | "json"
            | "jsonc"
            | "json5"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "properties"
            | "dockerfile"
            | "makefile"
            | "cmake"
            | "proto"
            | "thrift"
            | "avsc"
            | "tf"
            | "hcl"
            | "csv"
            | "tsv"
            | "tex"
            | "bib"
            | "el"
            | "lisp"
            | "clj"
            | "cljs"
            | "edn"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "hs"
            | "lhs"
            | "ml"
            | "mli"
            | "sml"
            | "v"
            | "sv"
            | "vhd"
            | "vhdl"
            | "zig"
            | "nim"
            | "d"
            | "dart"
            | "pas"
            | "lock"
            | "log"
            | "gitignore"
            | "editorconfig"
    )
}

/// Known extensionless text filenames (case-insensitive match on the file
/// name component). Handles files like `Dockerfile`, `Makefile`, etc. that
/// have no extension and would otherwise be treated as binary/unsupported.
pub fn is_text_filename(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_ascii_lowercase(),
        None => return false,
    };
    matches!(
        name.as_str(),
        "dockerfile"
            | "makefile"
            | "cmakelists.txt"
            | "vagrantfile"
            | "gemfile"
            | "rakefile"
            | "procfile"
            | "justfile"
            | "taskfile"
            | "brewfile"
            | "guardfile"
            | "berksfile"
            | "podfile"
            | "fastfile"
            | "appfile"
            | "matchfile"
            | "snapfile"
            | "dangerfile"
            | "license"
            | "licence"
            | "authors"
            | "contributors"
            | "changelog"
            | "readme"
            | "todo"
    )
}

/// Known binary file extensions that should never be treated as text.
fn is_binary_extension(ext: &str) -> bool {
    matches!(
        ext,
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "tiff" | "tif"
            | "psd" | "raw" | "cr2" | "nef" | "heic" | "heif" | "avif"
        // Audio
            | "mp3" | "wav" | "flac" | "aac" | "ogg" | "wma" | "m4a" | "opus"
        // Video
            | "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" | "m4v"
        // Archives
            | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst"
        // Compiled / object
            | "o" | "obj" | "so" | "dylib" | "dll" | "exe" | "class" | "pyc" | "pyo"
            | "wasm"
        // Fonts
            | "ttf" | "otf" | "woff" | "woff2" | "eot"
        // Databases
            | "sqlite" | "db" | "sqlite3"
        // Documents (binary formats)
            | "doc" | "xls" | "ppt"
        // 3D models
            | "stl" | "3mf" | "glb"
        // Other binary
            | "bin" | "dat" | "iso" | "dmg" | "pkg" | "deb" | "rpm"
    )
}

/// Determine whether a file is binary.
///
/// Uses a three-tier strategy:
/// 1. Known text extensions / filenames → text
/// 2. Known binary extensions → binary
/// 3. Content sniffing: read the first 8 KB and look for null bytes
///    (the same heuristic used by git and GitHub)
pub fn is_binary_file(path: &Path) -> std::io::Result<bool> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

    // SVG is an image MIME type but it's actually XML text.
    if ext == "svg" {
        return Ok(false);
    }

    if !ext.is_empty() && is_text_extension(&ext) {
        return Ok(false);
    }
    if is_text_filename(path) {
        return Ok(false);
    }
    if !ext.is_empty() && is_binary_extension(&ext) {
        return Ok(true);
    }

    // Fallback: sniff first 8 KB for null bytes.
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 8192];
    let n = reader.read(&mut buf)?;
    Ok(buf[..n].contains(&0))
}

/// Return a MIME type for the given lowercase file extension.
pub fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        // SVG is text-based XML but has a specific image MIME type
        "svg" => "image/svg+xml",
        // Text / code
        e if is_text_extension(e) => "text/plain",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "heic" | "heif" => "image/heif",
        "avif" => "image/avif",
        // PDF
        "pdf" => "application/pdf",
        // Archives
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        // 3D models
        "stl" => "model/stl",
        "3mf" => "model/3mf",
        "glb" => "model/gltf-binary",
        // Fonts
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        // Fallback
        _ => "application/octet-stream",
    }
}

/// Extract text from a PDF file.
fn extract_pdf(path: &Path) -> anyhow::Result<Option<String>> {
    let bytes = std::fs::read(path)?;
    match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                Ok(None) // scanned PDF with no extractable text
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(e) => {
            tracing::debug!(path = %path.display(), "PDF extraction failed: {e}");
            Ok(None) // treat parse failures as unsupported
        }
    }
}

/// Extract text from an Apple iWork file (.pages, .numbers, .key).
///
/// Uses a two-stage strategy:
/// 1. **Native IWA parsing** — decodes the Snappy-compressed protobuf
///    archives and extracts text from `TSWP.StorageArchive` messages.
/// 2. **Preview.pdf fallback** — if native parsing yields no text, tries
///    the embedded `QuickLook/Preview.pdf` via pdf-extract.
///
/// Returns `Ok(None)` when neither method produces extractable text.
fn extract_iwork(path: &Path) -> anyhow::Result<Option<String>> {
    // Stage 1: try native IWA parsing
    match hive_iwork::extract_text(path) {
        Ok(Some(text)) => return Ok(Some(text)),
        Ok(None) => {
            tracing::debug!(
                path = %path.display(),
                "iWork native IWA parsing returned no text, trying Preview.pdf fallback"
            );
        }
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                "iWork native IWA parsing failed: {e}, trying Preview.pdf fallback"
            );
        }
    }

    // Stage 2: fall back to QuickLook/Preview.pdf
    extract_iwork_preview(path)
}

/// Fallback: extract text from the embedded `QuickLook/Preview.pdf`.
fn extract_iwork_preview(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut entry = match archive.by_name("QuickLook/Preview.pdf") {
        Ok(entry) => entry,
        Err(_) => {
            tracing::debug!(
                path = %path.display(),
                "iWork file has no QuickLook/Preview.pdf — cannot extract text"
            );
            return Ok(None);
        }
    };

    let mut pdf_bytes = Vec::new();
    entry.read_to_end(&mut pdf_bytes)?;

    match pdf_extract::extract_text_from_mem(&pdf_bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                "iWork Preview.pdf extraction failed: {e}"
            );
            Ok(None)
        }
    }
}

/// Extract text from a DOCX file (Office Open XML).
///
/// DOCX files are ZIP archives containing `word/document.xml` with the
/// main body text in `<w:t>` elements.
fn extract_docx(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut text = String::new();
    // Main document body
    if let Ok(mut entry) = archive.by_name("word/document.xml") {
        let mut xml = String::new();
        entry.read_to_string(&mut xml)?;
        extract_xml_text(&xml, "w:t", &mut text);
    }

    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

/// Extract text from a PPTX file (Office Open XML Presentation).
///
/// PPTX files are ZIP archives containing `ppt/slides/slide*.xml` with
/// text in `<a:t>` elements.
fn extract_pptx(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut text = String::new();
    let slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            let name = entry.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    for name in slide_names {
        if let Ok(mut entry) = archive.by_name(&name) {
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
            extract_xml_text(&xml, "a:t", &mut text);
            text.push('\n');
        }
    }

    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

/// Extract text from an XLSX file (Office Open XML Spreadsheet).
///
/// XLSX files are ZIP archives. Cell text lives in per-sheet worksheet XML
/// files, with string values stored in a shared-string table.
fn extract_xlsx(path: &Path) -> anyhow::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;

    let shared_strings = xlsx_read_shared_strings(&mut archive);
    let sheets = xlsx_read_sheet_info(&mut archive);

    if sheets.is_empty() {
        return Ok(None);
    }

    let rels = xlsx_read_workbook_rels(&mut archive);

    let mut output = String::new();
    for (i, (sheet_name, r_id)) in sheets.iter().enumerate() {
        // Resolve the relationship ID to a file path; fall back to the
        // conventional `sheet{i+1}.xml` naming when rels are unavailable.
        let sheet_file = rels
            .get(r_id.as_str())
            .map(|target| {
                if let Some(stripped) = target.strip_prefix('/') {
                    // Absolute path in the archive — strip leading slash.
                    stripped.to_string()
                } else {
                    format!("xl/{target}")
                }
            })
            .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", i + 1));

        let rows = xlsx_read_sheet_rows(&mut archive, &sheet_file, &shared_strings);
        if rows.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("=== Sheet: {sheet_name} ===\n"));
        for row in &rows {
            output.push_str(&row.join("\t"));
            output.push('\n');
        }
    }

    if output.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(output))
    }
}

/// Read the shared-string table from `xl/sharedStrings.xml`.
fn xlsx_read_shared_strings(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let Ok(mut entry) = archive.by_name("xl/sharedStrings.xml") else {
        return vec![];
    };
    let mut xml = String::new();
    if entry.read_to_string(&mut xml).is_err() {
        return vec![];
    }

    let mut reader = Reader::from_str(&xml);
    let mut strings = Vec::new();
    let mut in_si = false;
    let mut current = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                if local == "si" {
                    in_si = true;
                    current.clear();
                }
            }
            Ok(Event::Text(ref e)) if in_si => {
                if let Ok(t) = e.unescape() {
                    current.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                if local == "si" {
                    strings.push(current.clone());
                    in_si = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    strings
}

/// Read sheet names and their relationship IDs from `xl/workbook.xml`.
///
/// Returns `(name, rId)` pairs in document order.
fn xlsx_read_sheet_info(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Vec<(String, String)> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let Ok(mut entry) = archive.by_name("xl/workbook.xml") else {
        return vec![];
    };
    let mut xml = String::new();
    if entry.read_to_string(&mut xml).is_err() {
        return vec![];
    }

    let mut reader = Reader::from_str(&xml);
    let mut sheets = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                if local == "sheet" {
                    let mut name = String::new();
                    let mut r_id = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"name" {
                            name = String::from_utf8_lossy(&attr.value).to_string();
                        }
                        let key = attr.key.as_ref();
                        // The rId attribute uses the `r:` namespace prefix.
                        if key == b"r:id" || key.ends_with(b":id") || xlsx_local_name(key) == "id" {
                            r_id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                    sheets.push((name, r_id));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    sheets
}

/// Read the workbook relationship file `xl/_rels/workbook.xml.rels`.
///
/// Returns a map from relationship ID (e.g. `rId1`) to the target path
/// (relative to `xl/`).
fn xlsx_read_workbook_rels(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> std::collections::HashMap<String, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let Ok(mut entry) = archive.by_name("xl/_rels/workbook.xml.rels") else {
        return std::collections::HashMap::new();
    };
    let mut xml = String::new();
    if entry.read_to_string(&mut xml).is_err() {
        return std::collections::HashMap::new();
    }

    let mut reader = Reader::from_str(&xml);
    let mut rels = std::collections::HashMap::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                if local == "Relationship" {
                    let mut id = String::new();
                    let mut target = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"Id" {
                            id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                        if attr.key.as_ref() == b"Target" {
                            target = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                    if !id.is_empty() && !target.is_empty() {
                        rels.insert(id, target);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    rels
}

/// Read rows from a single worksheet XML, resolving cell values.
///
/// Returns at most 500 rows. Each row is a `Vec<String>` where the position
/// is derived from the cell's column letter (A=0, B=1, …).
fn xlsx_read_sheet_rows(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
    sheet_path: &str,
    shared_strings: &[String],
) -> Vec<Vec<String>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    const MAX_ROWS: usize = 500;

    let Ok(mut entry) = archive.by_name(sheet_path) else {
        return vec![];
    };
    let mut xml = String::new();
    if entry.read_to_string(&mut xml).is_err() {
        return vec![];
    }

    let mut reader = Reader::from_str(&xml);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut in_row = false;
    let mut current_row: Vec<(usize, String)> = Vec::new(); // (col_index, value)
    let mut cell_type = String::new();
    let mut cell_ref = String::new();
    let mut in_value = false;
    let mut in_inline_str = false;
    let mut cell_value = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                match local.as_str() {
                    "row" => {
                        if rows.len() >= MAX_ROWS {
                            break;
                        }
                        in_row = true;
                        current_row.clear();
                    }
                    "c" if in_row => {
                        cell_type.clear();
                        cell_ref.clear();
                        cell_value.clear();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"t" => {
                                    cell_type = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                b"r" => {
                                    cell_ref = String::from_utf8_lossy(&attr.value).to_string();
                                }
                                _ => {}
                            }
                        }
                    }
                    "v" if in_row => {
                        in_value = true;
                    }
                    "is" if in_row => {
                        in_inline_str = true;
                    }
                    "t" if in_inline_str => {
                        in_value = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if in_value => {
                if let Ok(t) = e.unescape() {
                    cell_value.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = xlsx_local_name(e.name().as_ref());
                match local.as_str() {
                    "v" => {
                        in_value = false;
                    }
                    "t" if in_inline_str => {
                        in_value = false;
                    }
                    "is" => {
                        in_inline_str = false;
                    }
                    "c" if in_row => {
                        let resolved = if cell_type == "s" {
                            cell_value
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|idx| shared_strings.get(idx).cloned())
                                .unwrap_or_else(|| cell_value.clone())
                        } else {
                            cell_value.clone()
                        };
                        let col = xlsx_col_index(&cell_ref);
                        current_row.push((col, resolved));
                    }
                    "row" => {
                        if in_row && !current_row.is_empty() {
                            let max_col = current_row.iter().map(|(c, _)| *c).max().unwrap_or(0);
                            let mut row_vec = vec![String::new(); max_col + 1];
                            for (col, val) in &current_row {
                                row_vec[*col] = val.clone();
                            }
                            rows.push(row_vec);
                        }
                        in_row = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    rows
}

/// Parse column letters from a cell reference like "B3" → column index 1.
fn xlsx_col_index(cell_ref: &str) -> usize {
    const MAX_EXCEL_COL: usize = 16_384; // Excel max column is XFD = 16383
    let mut col: usize = 0;
    for b in cell_ref.bytes() {
        if b.is_ascii_alphabetic() {
            let next = col
                .checked_mul(26)
                .and_then(|c| c.checked_add((b.to_ascii_uppercase() - b'A') as usize + 1));
            match next {
                Some(n) if n <= MAX_EXCEL_COL => col = n,
                _ => return 0, // overflow or exceeds max — clamp to column A
            }
        } else {
            break;
        }
    }
    col.saturating_sub(1) // A=0, B=1, …
}

/// Strip XML namespace prefix: "x:sheet" → "sheet".
fn xlsx_local_name(full: &[u8]) -> String {
    let s = std::str::from_utf8(full).unwrap_or("");
    s.rsplit_once(':').map_or(s, |(_, local)| local).to_string()
}

/// Extract text content from XML `<tag>...</tag>` elements using quick-xml.
fn extract_xml_text(xml: &str, tag_name: &str, out: &mut String) {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut in_target = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local_name = e.local_name();
                let local = String::from_utf8_lossy(local_name.as_ref());
                // Match both prefixed (w:t) and unprefixed (t) forms
                if local == tag_name
                    || tag_name.split(':').next_back().is_some_and(|short| local == short)
                {
                    in_target = true;
                }
            }
            Ok(Event::Text(ref e)) if in_target => {
                if let Ok(t) = e.unescape() {
                    out.push_str(&t);
                }
            }
            Ok(Event::End(ref e)) => {
                let local_name = e.local_name();
                let local = String::from_utf8_lossy(local_name.as_ref());
                if local == tag_name
                    || tag_name.split(':').next_back().is_some_and(|short| local == short)
                {
                    in_target = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_extension_detects_common_types() {
        assert!(is_text_extension("rs"));
        assert!(is_text_extension("py"));
        assert!(is_text_extension("md"));
        assert!(is_text_extension("json"));
        assert!(is_text_extension("yaml"));
        assert!(!is_text_extension("png"));
        assert!(!is_text_extension("exe"));
        assert!(!is_text_extension("mp4"));
    }

    #[test]
    fn extract_text_reads_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "Hello, world!").unwrap();
        let result = extract_text(&file).unwrap();
        assert_eq!(result, Some("Hello, world!".to_string()));
    }

    #[test]
    fn extract_text_returns_none_for_binary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("image.png");
        std::fs::write(&file, [0x89, 0x50, 0x4e, 0x47]).unwrap();
        let result = extract_text(&file).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn extract_xml_text_extracts_tagged_content() {
        let xml = r#"<root><w:t>Hello</w:t><w:t> World</w:t></root>"#;
        let mut out = String::new();
        extract_xml_text(xml, "w:t", &mut out);
        assert_eq!(out, "Hello World");
    }

    #[test]
    fn xlsx_col_index_parses_column_letters() {
        assert_eq!(xlsx_col_index("A1"), 0);
        assert_eq!(xlsx_col_index("B3"), 1);
        assert_eq!(xlsx_col_index("C10"), 2);
        assert_eq!(xlsx_col_index("Z1"), 25);
        assert_eq!(xlsx_col_index("AA1"), 26);
    }

    /// Helper: build a minimal XLSX (ZIP) in memory with the given XML parts.
    fn make_xlsx(
        shared_strings_xml: &str,
        workbook_xml: &str,
        sheets: &[(&str, &str)], // (zip path, xml content)
    ) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            z.start_file("xl/sharedStrings.xml", opts).unwrap();
            z.write_all(shared_strings_xml.as_bytes()).unwrap();

            z.start_file("xl/workbook.xml", opts).unwrap();
            z.write_all(workbook_xml.as_bytes()).unwrap();

            for (path, xml) in sheets {
                z.start_file(path.to_string(), opts).unwrap();
                z.write_all(xml.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_xlsx_basic() {
        let shared = r#"<?xml version="1.0"?><sst><si><t>Name</t></si><si><t>Age</t></si><si><t>Alice</t></si><si><t>Bob</t></si></sst>"#;
        let workbook =
            r#"<?xml version="1.0"?><workbook><sheets><sheet name="People"/></sheets></workbook>"#;
        let sheet1 = r#"<?xml version="1.0"?>
<worksheet>
  <sheetData>
    <row r="1">
      <c r="A1" t="s"><v>0</v></c>
      <c r="B1" t="s"><v>1</v></c>
    </row>
    <row r="2">
      <c r="A2" t="s"><v>2</v></c>
      <c r="B2"><v>30</v></c>
    </row>
    <row r="3">
      <c r="A3" t="s"><v>3</v></c>
      <c r="B3"><v>25</v></c>
    </row>
  </sheetData>
</worksheet>"#;

        let xlsx_bytes = make_xlsx(shared, workbook, &[("xl/worksheets/sheet1.xml", sheet1)]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.xlsx");
        std::fs::write(&path, &xlsx_bytes).unwrap();

        let result = extract_xlsx(&path).unwrap().expect("should return Some");
        assert!(result.contains("=== Sheet: People ==="));
        assert!(result.contains("Name\tAge"));
        assert!(result.contains("Alice\t30"));
        assert!(result.contains("Bob\t25"));
    }

    #[test]
    fn extract_xlsx_inline_strings() {
        let shared = r#"<?xml version="1.0"?><sst></sst>"#;
        let workbook =
            r#"<?xml version="1.0"?><workbook><sheets><sheet name="Data"/></sheets></workbook>"#;
        let sheet1 = r#"<?xml version="1.0"?>
<worksheet>
  <sheetData>
    <row r="1">
      <c r="A1" t="inlineStr"><is><t>Hello</t></is></c>
      <c r="B1" t="inlineStr"><is><t>World</t></is></c>
    </row>
  </sheetData>
</worksheet>"#;

        let xlsx_bytes = make_xlsx(shared, workbook, &[("xl/worksheets/sheet1.xml", sheet1)]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inline.xlsx");
        std::fs::write(&path, &xlsx_bytes).unwrap();

        let result = extract_xlsx(&path).unwrap().expect("should return Some");
        assert!(result.contains("=== Sheet: Data ==="));
        assert!(result.contains("Hello\tWorld"));
    }

    #[test]
    fn extract_xlsx_multiple_sheets() {
        let shared = r#"<?xml version="1.0"?><sst><si><t>A</t></si><si><t>B</t></si></sst>"#;
        let workbook = r#"<?xml version="1.0"?><workbook><sheets><sheet name="First"/><sheet name="Second"/></sheets></workbook>"#;
        let sheet1 = r#"<?xml version="1.0"?><worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#;
        let sheet2 = r#"<?xml version="1.0"?><worksheet><sheetData><row r="1"><c r="A1" t="s"><v>1</v></c></row></sheetData></worksheet>"#;

        let xlsx_bytes = make_xlsx(
            shared,
            workbook,
            &[("xl/worksheets/sheet1.xml", sheet1), ("xl/worksheets/sheet2.xml", sheet2)],
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.xlsx");
        std::fs::write(&path, &xlsx_bytes).unwrap();

        let result = extract_xlsx(&path).unwrap().expect("should return Some");
        assert!(result.contains("=== Sheet: First ==="));
        assert!(result.contains("=== Sheet: Second ==="));
        assert!(result.contains("A\n"));
        assert!(result.contains("B\n"));
    }

    #[test]
    fn extract_text_routes_xlsx() {
        let shared = r#"<?xml version="1.0"?><sst><si><t>Val</t></si></sst>"#;
        let workbook =
            r#"<?xml version="1.0"?><workbook><sheets><sheet name="S"/></sheets></workbook>"#;
        let sheet1 = r#"<?xml version="1.0"?><worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#;

        let xlsx_bytes = make_xlsx(shared, workbook, &[("xl/worksheets/sheet1.xml", sheet1)]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routed.xlsx");
        std::fs::write(&path, &xlsx_bytes).unwrap();

        let result = extract_text(&path).unwrap().expect("extract_text should route xlsx");
        assert!(result.contains("Val"));
    }

    #[test]
    fn extract_text_handles_dockerfile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Dockerfile");
        std::fs::write(&file, "FROM rust:latest\nRUN cargo build").unwrap();
        let result = extract_text(&file).unwrap();
        assert_eq!(result, Some("FROM rust:latest\nRUN cargo build".to_string()));
    }

    #[test]
    fn extract_text_handles_makefile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Makefile");
        std::fs::write(&file, "all:\n\techo hello").unwrap();
        let result = extract_text(&file).unwrap();
        assert_eq!(result, Some("all:\n\techo hello".to_string()));
    }

    #[test]
    fn is_text_filename_detects_known_names() {
        assert!(is_text_filename(Path::new("/tmp/Dockerfile")));
        assert!(is_text_filename(Path::new("/tmp/Makefile")));
        assert!(is_text_filename(Path::new("/tmp/LICENSE")));
        assert!(is_text_filename(Path::new("/tmp/Procfile")));
        assert!(!is_text_filename(Path::new("/tmp/unknown_binary")));
        assert!(!is_text_filename(Path::new("/tmp/random")));
    }

    /// Helper: build a minimal iWork-style ZIP containing a `QuickLook/Preview.pdf`.
    ///
    /// The embedded PDF is a tiny valid PDF that contains the given text.
    fn make_iwork_with_preview(text: &str) -> Vec<u8> {
        // Build a minimal valid PDF with the given text.
        let stream = format!("BT /F1 12 Tf 100 700 Td ({text}) Tj ET");
        let pdf_content = format!(
            "%PDF-1.0\n\
             1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
             2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
             3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n\
             4 0 obj<</Length {}>>stream\n{stream}\nendstream\nendobj\n\
             5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
             xref\n0 6\ntrailer<</Size 6/Root 1 0 R>>\nstartxref\n0\n%%EOF",
            stream.len(),
        );

        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("QuickLook/Preview.pdf", opts).unwrap();
            z.write_all(pdf_content.as_bytes()).unwrap();
            // Add a dummy Index dir entry to mimic real iWork structure
            z.add_directory("Index", opts).unwrap();
            z.finish().unwrap();
        }
        buf
    }

    /// Helper: build a minimal iWork-style ZIP without a QuickLook preview.
    fn make_iwork_without_preview() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("Index/Document.iwa", opts).unwrap();
            z.write_all(b"dummy iwa content").unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_text_routes_iwork_pages() {
        // Verify .pages files are routed through extract_iwork_preview (not rejected).
        // We use a ZIP with a dummy Preview.pdf; pdf_extract may not parse our
        // minimal PDF, but the function should return Ok (not Err).
        let bytes = make_iwork_with_preview("Hello from Pages");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pages");
        std::fs::write(&path, &bytes).unwrap();

        let result = extract_text(&path);
        assert!(result.is_ok(), ".pages should be routed (not 'unsupported')");
    }

    #[test]
    fn extract_text_routes_iwork_numbers() {
        let bytes = make_iwork_with_preview("Spreadsheet data");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.numbers");
        std::fs::write(&path, &bytes).unwrap();

        let result = extract_text(&path);
        assert!(result.is_ok(), ".numbers should be routed (not 'unsupported')");
    }

    #[test]
    fn extract_text_routes_iwork_keynote() {
        let bytes = make_iwork_with_preview("Slide content here");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("presentation.key");
        std::fs::write(&path, &bytes).unwrap();

        let result = extract_text(&path);
        assert!(result.is_ok(), ".key should be routed (not 'unsupported')");
    }

    #[test]
    fn extract_iwork_returns_none_without_preview() {
        let bytes = make_iwork_without_preview();
        let dir = tempfile::tempdir().unwrap();

        for ext in &["pages", "numbers", "key"] {
            let path = dir.path().join(format!("test.{ext}"));
            std::fs::write(&path, &bytes).unwrap();
            let result = extract_text(&path).unwrap();
            assert_eq!(result, None, ".{ext} without Preview.pdf should return None");
        }
    }

    #[test]
    fn is_binary_file_known_text_extensions() {
        let dir = tempfile::tempdir().unwrap();
        for ext in &["rs", "py", "go", "java", "cpp", "ts", "jsx", "env", "gitignore", "lock"] {
            let path = dir.path().join(format!("test.{ext}"));
            std::fs::write(&path, "fn main() {}").unwrap();
            assert!(!is_binary_file(&path).unwrap(), ".{ext} should be text");
        }
    }

    #[test]
    fn is_binary_file_known_binary_extensions() {
        let dir = tempfile::tempdir().unwrap();
        for ext in &["png", "jpg", "exe", "zip", "wasm", "mp4", "ttf"] {
            let path = dir.path().join(format!("test.{ext}"));
            std::fs::write(&path, [0x00, 0x01, 0x02, 0x03]).unwrap();
            assert!(is_binary_file(&path).unwrap(), ".{ext} should be binary");
        }
    }

    #[test]
    fn is_binary_file_svg_is_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.svg");
        std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#).unwrap();
        assert!(!is_binary_file(&path).unwrap(), "SVG should be text");
    }

    #[test]
    fn is_binary_file_extensionless_text_filenames() {
        let dir = tempfile::tempdir().unwrap();
        for name in &["Dockerfile", "Makefile", "LICENSE", "README"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "some content").unwrap();
            assert!(!is_binary_file(&path).unwrap(), "{name} should be text");
        }
    }

    #[test]
    fn is_binary_file_content_sniff_unknown_ext_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.xyz");
        std::fs::write(&path, "this is plain text with no null bytes").unwrap();
        assert!(!is_binary_file(&path).unwrap(), "text content with unknown ext should be text");
    }

    #[test]
    fn is_binary_file_content_sniff_unknown_ext_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.xyz");
        std::fs::write(&path, b"header\x00\x01\x02binary").unwrap();
        assert!(is_binary_file(&path).unwrap(), "binary content with unknown ext should be binary");
    }

    #[test]
    fn is_binary_file_empty_file_is_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.xyz");
        std::fs::write(&path, "").unwrap();
        assert!(!is_binary_file(&path).unwrap(), "empty file should be text");
    }

    #[test]
    fn mime_for_extension_covers_common_types() {
        assert_eq!(mime_for_extension("rs"), "text/plain");
        assert_eq!(mime_for_extension("go"), "text/plain");
        assert_eq!(mime_for_extension("dart"), "text/plain");
        assert_eq!(mime_for_extension("svg"), "image/svg+xml");
        assert_eq!(mime_for_extension("png"), "image/png");
        assert_eq!(mime_for_extension("pdf"), "application/pdf");
        assert_eq!(mime_for_extension("unknown"), "application/octet-stream");
    }
}
