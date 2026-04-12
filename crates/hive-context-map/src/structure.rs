use std::path::Path;

/// A single structural element extracted from a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureEntry {
    /// 1-based line number (for text files) or logical position (for binary docs).
    pub line: usize,
    /// Human-readable label, e.g. `"# Heading"`, `"fn main"`, `"Slide 1: Title"`.
    pub label: String,
}

/// Extract structural outline entries from file content.
///
/// For code files this uses tree-sitter grammars; for markdown it uses heading
/// regex; for office documents it parses internal XML.
pub fn extract_structure(path: &Path, content: &str, extension: &str) -> Vec<StructureEntry> {
    match extension {
        "md" | "markdown" => extract_markdown_headings(content),
        "rs" => extract_with_treesitter(content, Lang::Rust),
        "py" => extract_with_treesitter(content, Lang::Python),
        "js" | "jsx" | "mjs" | "cjs" => extract_with_treesitter(content, Lang::JavaScript),
        "ts" | "tsx" | "mts" | "cts" => extract_with_treesitter(content, Lang::TypeScript),
        "java" => extract_with_treesitter(content, Lang::Java),
        "go" => extract_with_treesitter(content, Lang::Go),
        "c" | "h" => extract_with_treesitter(content, Lang::C),
        "cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx" => extract_with_treesitter(content, Lang::Cpp),
        "cs" => extract_with_treesitter(content, Lang::CSharp),
        "docx" => extract_docx_headings(path),
        "pptx" => extract_pptx_slides(path),
        "xlsx" => extract_xlsx_structure(path),
        "csv" => extract_csv_headers(content),
        "tsv" => extract_tsv_headers(content),
        "pdf" => extract_pdf_pages(path),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

fn extract_markdown_headings(content: &str) -> Vec<StructureEntry> {
    let mut entries = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            // Count heading level and extract text
            let hashes: String = trimmed.chars().take_while(|&c| c == '#').collect();
            let text = trimmed[hashes.len()..].trim();
            if !text.is_empty() && hashes.len() <= 6 {
                entries.push(StructureEntry { line: i + 1, label: format!("{hashes} {text}") });
            }
        }
    }
    entries
}

// ---------------------------------------------------------------------------
// Tree-sitter based extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Go,
    C,
    Cpp,
    CSharp,
}

/// The node kinds we care about for each language, plus how to extract a label.
fn interesting_node_kinds(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &[
            "function_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "impl_item",
            "mod_item",
            "type_item",
            "const_item",
            "static_item",
            "macro_definition",
        ],
        Lang::Python => &["function_definition", "class_definition"],
        Lang::JavaScript => {
            &["function_declaration", "class_declaration", "method_definition", "export_statement"]
        }
        Lang::TypeScript => &[
            "function_declaration",
            "class_declaration",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
            "method_definition",
            "export_statement",
        ],
        Lang::Java => &[
            "class_declaration",
            "interface_declaration",
            "enum_declaration",
            "method_declaration",
            "constructor_declaration",
        ],
        Lang::Go => &["function_declaration", "method_declaration", "type_declaration"],
        Lang::C => &["function_definition", "struct_specifier", "enum_specifier"],
        Lang::Cpp => &[
            "function_definition",
            "class_specifier",
            "struct_specifier",
            "enum_specifier",
            "namespace_definition",
        ],
        Lang::CSharp => &[
            "class_declaration",
            "interface_declaration",
            "struct_declaration",
            "enum_declaration",
            "method_declaration",
            "namespace_declaration",
            "constructor_declaration",
        ],
    }
}

fn get_language(lang: Lang) -> tree_sitter::Language {
    match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
    }
}

fn extract_with_treesitter(content: &str, lang: Lang) -> Vec<StructureEntry> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&get_language(lang)).is_err() {
        return vec![];
    }

    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return vec![],
    };

    let interesting = interesting_node_kinds(lang);
    let bytes = content.as_bytes();
    let mut entries = Vec::new();

    collect_nodes(tree.root_node(), interesting, bytes, lang, &mut entries, 0);
    entries
}

/// Max nesting depth to prevent runaway recursion on pathological inputs.
const MAX_DEPTH: usize = 32;

fn collect_nodes(
    node: tree_sitter::Node,
    interesting: &[&str],
    source: &[u8],
    lang: Lang,
    entries: &mut Vec<StructureEntry>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }

    let kind = node.kind();

    if interesting.contains(&kind) {
        if let Some(label) = build_label(node, source, lang) {
            entries.push(StructureEntry { line: node.start_position().row + 1, label });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // For export_statement, only recurse if the export itself wasn't labelled
        collect_nodes(child, interesting, source, lang, entries, depth + 1);
    }
}

fn build_label(node: tree_sitter::Node, source: &[u8], lang: Lang) -> Option<String> {
    let kind = node.kind();
    match lang {
        Lang::Rust => build_rust_label(node, source, kind),
        Lang::Python => build_python_label(node, source, kind),
        Lang::JavaScript | Lang::TypeScript => build_js_ts_label(node, source, kind, lang),
        Lang::Java => build_java_label(node, source, kind),
        Lang::Go => build_go_label(node, source, kind),
        Lang::C | Lang::Cpp => build_c_cpp_label(node, source, kind),
        Lang::CSharp => build_csharp_label(node, source, kind),
    }
}

fn node_text<'a>(node: tree_sitter::Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn child_field_text<'a>(node: tree_sitter::Node, field: &str, source: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name(field).map(|n| node_text(n, source))
}

// ---- Rust ----
fn build_rust_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "function_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("fn {name}"))
        }
        "struct_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("struct {name}"))
        }
        "enum_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("enum {name}"))
        }
        "trait_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("trait {name}"))
        }
        "impl_item" => {
            // Try to get the type being implemented
            let type_node = node.child_by_field_name("type")?;
            let type_text = node_text(type_node, source);
            // Check for trait impl: `impl Trait for Type`
            if let Some(trait_node) = node.child_by_field_name("trait") {
                let trait_text = node_text(trait_node, source);
                Some(format!("impl {trait_text} for {type_text}"))
            } else {
                Some(format!("impl {type_text}"))
            }
        }
        "mod_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("mod {name}"))
        }
        "type_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("type {name}"))
        }
        "const_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("const {name}"))
        }
        "static_item" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("static {name}"))
        }
        "macro_definition" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("macro {name}"))
        }
        _ => None,
    }
}

// ---- Python ----
fn build_python_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "function_definition" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("def {name}"))
        }
        "class_definition" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("class {name}"))
        }
        _ => None,
    }
}

// ---- JS / TS ----
fn build_js_ts_label(
    node: tree_sitter::Node,
    source: &[u8],
    kind: &str,
    lang: Lang,
) -> Option<String> {
    match kind {
        "function_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("function {name}"))
        }
        "class_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("class {name}"))
        }
        "interface_declaration" if matches!(lang, Lang::TypeScript) => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("interface {name}"))
        }
        "type_alias_declaration" if matches!(lang, Lang::TypeScript) => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("type {name}"))
        }
        "enum_declaration" if matches!(lang, Lang::TypeScript) => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("enum {name}"))
        }
        "method_definition" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("method {name}"))
        }
        "export_statement" => {
            // Only label if it's a default export or re-export, not if it wraps
            // a declaration (which will be labelled by its own node).
            let has_declaration = node.child_by_field_name("declaration").is_some();
            if has_declaration {
                return None;
            }
            // e.g. `export default ...` or `export { ... }`
            let text = node_text(node, source);
            let preview: String = text.chars().take(60).collect();
            Some(preview.trim().to_string())
        }
        _ => None,
    }
}

// ---- Java ----
fn build_java_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "class_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("class {name}"))
        }
        "interface_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("interface {name}"))
        }
        "enum_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("enum {name}"))
        }
        "method_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("method {name}"))
        }
        "constructor_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("constructor {name}"))
        }
        _ => None,
    }
}

// ---- Go ----
fn build_go_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "function_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("func {name}"))
        }
        "method_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("func {name}"))
        }
        "type_declaration" => {
            // type_declaration contains type_spec children
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_spec" {
                    if let Some(name) = child_field_text(child, "name", source) {
                        let type_node = child.child_by_field_name("type");
                        let type_kind = type_node.map(|n| n.kind()).unwrap_or("");
                        let keyword = match type_kind {
                            "struct_type" => "struct",
                            "interface_type" => "interface",
                            _ => "type",
                        };
                        return Some(format!("{keyword} {name}"));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// ---- C / C++ ----
fn build_c_cpp_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "function_definition" => {
            let declarator = node.child_by_field_name("declarator")?;
            // The declarator might be a function_declarator containing the name
            let name_node = declarator.child_by_field_name("declarator").unwrap_or(declarator);
            let name = node_text(name_node, source);
            Some(format!("fn {name}"))
        }
        "class_specifier" | "struct_specifier" => {
            let name = child_field_text(node, "name", source)?;
            let keyword = if kind == "class_specifier" { "class" } else { "struct" };
            Some(format!("{keyword} {name}"))
        }
        "enum_specifier" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("enum {name}"))
        }
        "namespace_definition" => {
            let name = child_field_text(node, "name", source).unwrap_or("(anonymous)");
            Some(format!("namespace {name}"))
        }
        _ => None,
    }
}

// ---- C# ----
fn build_csharp_label(node: tree_sitter::Node, source: &[u8], kind: &str) -> Option<String> {
    match kind {
        "class_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("class {name}"))
        }
        "interface_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("interface {name}"))
        }
        "struct_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("struct {name}"))
        }
        "enum_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("enum {name}"))
        }
        "method_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("method {name}"))
        }
        "namespace_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("namespace {name}"))
        }
        "constructor_declaration" => {
            let name = child_field_text(node, "name", source)?;
            Some(format!("constructor {name}"))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Office document extraction
// ---------------------------------------------------------------------------

fn extract_docx_headings(path: &Path) -> Vec<StructureEntry> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return vec![];
    };
    let Ok(mut doc_xml) = archive.by_name("word/document.xml") else {
        return vec![];
    };

    let mut xml_data = String::new();
    if std::io::Read::read_to_string(&mut doc_xml, &mut xml_data).is_err() {
        return vec![];
    }

    parse_docx_headings_from_xml(&xml_data)
}

fn parse_docx_headings_from_xml(xml: &str) -> Vec<StructureEntry> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut inside_paragraph = false;
    let mut current_heading_level: Option<String> = None;
    let mut current_text = String::new();
    let mut heading_counter = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                match local {
                    "p" => {
                        inside_paragraph = true;
                        current_heading_level = None;
                        current_text.clear();
                    }
                    "pStyle" if inside_paragraph => {
                        // Look for val attribute indicating a heading style
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"w:val" || attr.key.as_ref() == b"val" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if val.starts_with("Heading") || val.starts_with("heading") {
                                    current_heading_level = Some(val);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if inside_paragraph => {
                if let Ok(text) = e.unescape() {
                    current_text.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                if local == "p" && inside_paragraph {
                    if let Some(ref level) = current_heading_level {
                        let text = current_text.trim().to_string();
                        if !text.is_empty() {
                            heading_counter += 1;
                            entries.push(StructureEntry {
                                line: heading_counter,
                                label: format!("{level}: {text}"),
                            });
                        }
                    }
                    inside_paragraph = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    entries
}

fn extract_pptx_slides(path: &Path) -> Vec<StructureEntry> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return vec![];
    };

    // Collect slide file names and sort them
    let mut slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let name = archive.by_index(i).ok()?.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    slide_names.sort();

    let mut entries = Vec::new();
    for (i, slide_name) in slide_names.iter().enumerate() {
        let slide_num = i + 1;
        if let Ok(mut slide_file) = archive.by_name(slide_name) {
            let mut xml = String::new();
            if std::io::Read::read_to_string(&mut slide_file, &mut xml).is_ok() {
                let title = extract_slide_title(&xml);
                if let Some(title) = title {
                    entries.push(StructureEntry {
                        line: slide_num,
                        label: format!("Slide {slide_num}: {title}"),
                    });
                } else {
                    entries.push(StructureEntry {
                        line: slide_num,
                        label: format!("Slide {slide_num}"),
                    });
                }
            }
        }
    }
    entries
}

fn extract_slide_title(xml: &str) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut in_title_shape = false;
    let mut in_text_body = false;
    let mut depth = 0u32;
    let mut title_text = String::new();

    // Look for a shape with type="title" or "ctrTitle"
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                depth += 1;
                match local {
                    "sp" => {
                        // Reset state for each shape
                    }
                    "ph" => {
                        // Placeholder type
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                let val = String::from_utf8_lossy(&attr.value);
                                if val == "title" || val == "ctrTitle" {
                                    in_title_shape = true;
                                }
                            }
                        }
                    }
                    "txBody" if in_title_shape => {
                        in_text_body = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                if local == "ph" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"type" {
                            let val = String::from_utf8_lossy(&attr.value);
                            if val == "title" || val == "ctrTitle" {
                                in_title_shape = true;
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) if in_text_body => {
                if let Ok(text) = e.unescape() {
                    let t = text.trim();
                    if !t.is_empty() {
                        if !title_text.is_empty() {
                            title_text.push(' ');
                        }
                        title_text.push_str(t);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                depth = depth.saturating_sub(1);
                if local == "sp" {
                    if in_title_shape && !title_text.is_empty() {
                        return Some(title_text);
                    }
                    in_title_shape = false;
                    in_text_body = false;
                    title_text.clear();
                }
                if local == "txBody" {
                    in_text_body = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // Fallback: if no title placeholder found, get first text run
    None
}

fn extract_xlsx_structure(path: &Path) -> Vec<StructureEntry> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return vec![];
    };

    // 1. Read shared strings
    let shared_strings = read_xlsx_shared_strings(&mut archive);

    // 2. Read workbook for sheet names
    let sheet_names = read_xlsx_sheet_names(&mut archive);

    let mut entries = Vec::new();
    for (i, sheet_name) in sheet_names.iter().enumerate() {
        let sheet_file = format!("xl/worksheets/sheet{}.xml", i + 1);
        let headers = read_xlsx_first_row(&mut archive, &sheet_file, &shared_strings);
        if headers.is_empty() {
            entries.push(StructureEntry { line: i + 1, label: format!("Sheet: {sheet_name}") });
        } else {
            let cols = headers.join(", ");
            entries.push(StructureEntry {
                line: i + 1,
                label: format!("Sheet: {sheet_name} (columns: {cols})"),
            });
        }
    }
    entries
}

fn read_xlsx_shared_strings(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Vec<String> {
    let Ok(mut file) = archive.by_name("xl/sharedStrings.xml") else {
        return vec![];
    };
    let mut xml = String::new();
    if std::io::Read::read_to_string(&mut file, &mut xml).is_err() {
        return vec![];
    }

    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(&xml);
    let mut strings = Vec::new();
    let mut in_si = false;
    let mut current = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                if local == "si" {
                    in_si = true;
                    current.clear();
                }
            }
            Ok(Event::Text(ref e)) if in_si => {
                if let Ok(text) = e.unescape() {
                    current.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
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

fn read_xlsx_sheet_names(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Vec<String> {
    let Ok(mut file) = archive.by_name("xl/workbook.xml") else {
        return vec![];
    };
    let mut xml = String::new();
    if std::io::Read::read_to_string(&mut file, &mut xml).is_err() {
        return vec![];
    }

    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(&xml);
    let mut names = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                if local == "sheet" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"name" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            names.push(val);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    names
}

fn read_xlsx_first_row(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
    sheet_path: &str,
    shared_strings: &[String],
) -> Vec<String> {
    let Ok(mut file) = archive.by_name(sheet_path) else {
        return vec![];
    };
    let mut xml = String::new();
    if std::io::Read::read_to_string(&mut file, &mut xml).is_err() {
        return vec![];
    }

    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(&xml);
    let mut headers = Vec::new();
    let mut in_first_row = false;
    let mut row_count = 0u32;
    let mut cell_type = String::new();
    let mut in_value = false;
    let mut cell_value = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                match local {
                    "row" => {
                        row_count += 1;
                        if row_count == 1 {
                            in_first_row = true;
                        } else if in_first_row {
                            // We've moved past the first row
                            break;
                        }
                    }
                    "c" if in_first_row => {
                        cell_type.clear();
                        cell_value.clear();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"t" {
                                cell_type = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "v" if in_first_row => {
                        in_value = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if in_value => {
                if let Ok(text) = e.unescape() {
                    cell_value.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                let local = local.as_str();
                if local == "v" && in_first_row {
                    in_value = false;
                }
                if local == "c" && in_first_row {
                    // Resolve shared string reference
                    if cell_type == "s" {
                        if let Ok(idx) = cell_value.trim().parse::<usize>() {
                            if let Some(s) = shared_strings.get(idx) {
                                headers.push(s.clone());
                            } else {
                                headers.push(cell_value.clone());
                            }
                        } else {
                            headers.push(cell_value.clone());
                        }
                    } else {
                        headers.push(cell_value.clone());
                    }
                }
                if local == "row" && in_first_row {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    headers
}

fn extract_csv_headers(content: &str) -> Vec<StructureEntry> {
    if let Some(first_line) = content.lines().next() {
        let cols: Vec<&str> = first_line.split(',').map(|s| s.trim()).collect();
        if !cols.is_empty() && !cols[0].is_empty() {
            return vec![StructureEntry {
                line: 1,
                label: format!("Columns: {}", cols.join(", ")),
            }];
        }
    }
    vec![]
}

fn extract_tsv_headers(content: &str) -> Vec<StructureEntry> {
    if let Some(first_line) = content.lines().next() {
        let cols: Vec<&str> = first_line.split('\t').map(|s| s.trim()).collect();
        if !cols.is_empty() && !cols[0].is_empty() {
            return vec![StructureEntry {
                line: 1,
                label: format!("Columns: {}", cols.join(", ")),
            }];
        }
    }
    vec![]
}

fn extract_pdf_pages(path: &Path) -> Vec<StructureEntry> {
    let text = match hive_workspace_index::extract_text(path) {
        Ok(Some(t)) => t,
        _ => return vec![],
    };

    let mut entries = Vec::new();
    let mut cumulative_line: usize = 1;

    for (i, page) in text.split('\x0c').enumerate() {
        entries.push(StructureEntry { line: cumulative_line, label: format!("Page {}", i + 1) });
        cumulative_line += page.lines().count().max(1);
    }

    entries
}

/// Extract the local name from a potentially namespace-prefixed XML tag.
fn local_name(full: &[u8]) -> String {
    let s = std::str::from_utf8(full).unwrap_or("");
    s.rsplit_once(':').map_or(s, |(_, local)| local).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_headings() {
        let content = "# Title\nsome text\n## Sub\nmore\n### Deep\n";
        let entries = extract_structure(Path::new("test.md"), content, "md");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].line, 1);
        assert_eq!(entries[0].label, "# Title");
        assert_eq!(entries[1].line, 3);
        assert_eq!(entries[1].label, "## Sub");
        assert_eq!(entries[2].line, 5);
        assert_eq!(entries[2].label, "### Deep");
    }

    #[test]
    fn rust_structure() {
        let content = r#"
mod utils;

pub struct Config {
    name: String,
}

pub enum State {
    Active,
    Inactive,
}

pub trait Handler {
    fn handle(&self);
}

impl Config {
    pub fn new() -> Self {
        Config { name: String::new() }
    }
}

fn main() {
    println!("hello");
}
"#;
        let entries = extract_structure(Path::new("main.rs"), content, "rs");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"mod utils"));
        assert!(labels.contains(&"struct Config"));
        assert!(labels.contains(&"enum State"));
        assert!(labels.contains(&"trait Handler"));
        assert!(labels.contains(&"impl Config"));
        assert!(labels.contains(&"fn main"));
        // fn new is inside impl, should also be captured
        assert!(labels.contains(&"fn new"));
    }

    #[test]
    fn python_structure() {
        let content = r#"
class MyClass:
    def __init__(self):
        pass

    def method(self):
        pass

def standalone():
    pass
"#;
        let entries = extract_structure(Path::new("test.py"), content, "py");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"class MyClass"));
        assert!(labels.contains(&"def __init__"));
        assert!(labels.contains(&"def method"));
        assert!(labels.contains(&"def standalone"));
    }

    #[test]
    fn javascript_structure() {
        let content = r#"
function greet(name) {
    return `Hello ${name}`;
}

class Animal {
    constructor(name) {
        this.name = name;
    }
}
"#;
        let entries = extract_structure(Path::new("test.js"), content, "js");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"function greet"));
        assert!(labels.contains(&"class Animal"));
    }

    #[test]
    fn typescript_structure() {
        let content = r#"
interface User {
    name: string;
    age: number;
}

type ID = string | number;

enum Color {
    Red,
    Green,
    Blue,
}

function process(user: User): void {}

class Service {
    method(): void {}
}
"#;
        let entries = extract_structure(Path::new("test.ts"), content, "ts");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"interface User"));
        assert!(labels.contains(&"type ID"));
        assert!(labels.contains(&"enum Color"));
        assert!(labels.contains(&"function process"));
        assert!(labels.contains(&"class Service"));
    }

    #[test]
    fn java_structure() {
        let content = r#"
public class Main {
    public void doSomething() {}

    public Main() {}
}

interface Runnable {
    void run();
}

enum Status {
    ACTIVE, INACTIVE
}
"#;
        let entries = extract_structure(Path::new("Main.java"), content, "java");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"class Main"));
        assert!(labels.contains(&"method doSomething"));
        assert!(labels.contains(&"interface Runnable"));
        assert!(labels.contains(&"enum Status"));
    }

    #[test]
    fn go_structure() {
        let content = r#"
package main

func main() {}

func helper(x int) int {
    return x
}

type Config struct {
    Name string
}

type Handler interface {
    Handle()
}
"#;
        let entries = extract_structure(Path::new("main.go"), content, "go");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"func main"));
        assert!(labels.contains(&"func helper"));
        assert!(labels.contains(&"struct Config"));
        assert!(labels.contains(&"interface Handler"));
    }

    #[test]
    fn c_structure() {
        let content = r#"
struct Point {
    int x;
    int y;
};

int add(int a, int b) {
    return a + b;
}

int main() {
    return 0;
}
"#;
        let entries = extract_structure(Path::new("test.c"), content, "c");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"struct Point"));
        assert!(labels.iter().any(|l| l.contains("add")));
        assert!(labels.iter().any(|l| l.contains("main")));
    }

    #[test]
    fn csharp_structure() {
        let content = r#"
namespace MyApp {
    public class Program {
        public void Run() {}
        public Program() {}
    }

    public interface IService {
        void Execute();
    }

    public enum Level {
        Low,
        High
    }
}
"#;
        let entries = extract_structure(Path::new("Program.cs"), content, "cs");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"namespace MyApp"));
        assert!(labels.contains(&"class Program"));
        assert!(labels.contains(&"interface IService"));
        assert!(labels.contains(&"enum Level"));
    }

    #[test]
    fn csv_headers() {
        let content = "Name,Email,Department\nAlice,alice@co.com,Eng\n";
        let entries = extract_structure(Path::new("data.csv"), content, "csv");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Columns: Name, Email, Department");
    }

    #[test]
    fn tsv_headers() {
        let content = "Name\tEmail\tDepartment\nAlice\talice@co.com\tEng\n";
        let entries = extract_structure(Path::new("data.tsv"), content, "tsv");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Columns: Name, Email, Department");
    }

    #[test]
    fn unsupported_extension_returns_empty() {
        let content = "some random content";
        let entries = extract_structure(Path::new("test.xyz"), content, "xyz");
        assert!(entries.is_empty());
    }

    #[test]
    fn docx_heading_xml_parsing() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>Executive Summary</w:t></w:r>
    </w:p>
    <w:p>
      <w:r><w:t>Some body text</w:t></w:r>
    </w:p>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading2"/></w:pPr>
      <w:r><w:t>Background</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let entries = parse_docx_headings_from_xml(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "Heading1: Executive Summary");
        assert_eq!(entries[1].label, "Heading2: Background");
    }

    #[test]
    fn pdf_pages_nonexistent_file() {
        let entries = extract_pdf_pages(Path::new("/nonexistent/fake.pdf"));
        assert!(entries.is_empty());
    }
}
