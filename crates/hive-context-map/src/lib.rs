use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use arc_swap::ArcSwap;
use hive_contracts::{
    ChannelClass, ContextMapStrategy, ToolAnnotations, ToolApproval, ToolDefinition,
};
use hive_model::{CompletionMessage, CompletionRequest, ModelRouter, ModelRouterError};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

mod structure;
mod walker;

pub use structure::{extract_structure, StructureEntry};
pub use walker::{walk_workspace, FileEntry};

/// Maximum total output size for the context map (in bytes).
/// Prevents blowing up the LLM context window.
const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KB

// ── Workspace classification ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum WorkspaceType {
    Code,
    Documents,
    Mixed,
}

/// Well-known programming-language extensions (a subset of text extensions).
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "go", "c", "cpp", "h", "hpp", "cs", "rb", "php",
    "swift", "kt", "scala", "lua", "pl", "pm", "r", "jl", "zig", "nim", "ex", "exs", "erl", "hs",
    "ml", "mli", "fs", "fsi", "fsx", "clj", "cljs", "cljc", "v", "sv", "vhd", "vhdl", "sh", "bash",
    "zsh", "fish", "bat", "ps1", "psm1", "m", "mm", "d", "dart", "groovy", "vue", "svelte", "elm",
    "purs",
];

/// Document-oriented extensions (binary office / rich-text formats).
const DOCUMENT_EXTENSIONS: &[&str] =
    &["pdf", "docx", "pptx", "xlsx", "doc", "xls", "ppt", "odt", "ods", "odp", "rtf"];

fn classify_workspace(files: &[FileEntry]) -> WorkspaceType {
    if files.is_empty() {
        return WorkspaceType::Code;
    }

    let mut code_count: usize = 0;
    let mut doc_count: usize = 0;

    for file in files {
        let ext = file
            .absolute_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if CODE_EXTENSIONS.contains(&ext.as_str()) {
            code_count += 1;
        } else if DOCUMENT_EXTENSIONS.contains(&ext.as_str()) {
            doc_count += 1;
        }
    }

    let total = files.len();
    if code_count * 2 > total {
        WorkspaceType::Code
    } else if doc_count * 2 > total {
        WorkspaceType::Documents
    } else {
        WorkspaceType::Mixed
    }
}

/// Builds a textual context map of the workspace that gets appended to
/// the system prompt before each LLM invocation.
pub trait ContextMapProvider: Send + Sync {
    /// Produce a context string for the given workspace.
    /// Returns an empty string when no context is available.
    fn build_context(&self, workspace_path: &str) -> String;
}

/// Lightweight, general-purpose workspace context.
///
/// Enumerates all files in the workspace, extracts structural outlines
/// (headings, function/class/struct definitions), and formats a compact
/// summary suitable for appending to the system prompt.
pub struct GeneralContextMap;

impl ContextMapProvider for GeneralContextMap {
    fn build_context(&self, workspace_path: &str) -> String {
        let files = walk_workspace(workspace_path);
        if files.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Workspace Context Map\n\n");
        let mut truncated = false;

        for file in &files {
            let ext = file
                .absolute_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            let file_line = build_file_entry(file, &ext);

            // Check size budget before appending
            if output.len() + file_line.len() > MAX_OUTPUT_BYTES {
                truncated = true;
                break;
            }

            output.push_str(&file_line);
        }

        if truncated {
            output.push_str("\n... (truncated — workspace has more files)\n");
        }

        output
    }
}

/// Build the formatted entry for a single file, including its structure.
fn build_file_entry(file: &FileEntry, ext: &str) -> String {
    let mut entry = String::new();

    // Decide if this is a binary office doc or a text file
    match ext {
        "docx" | "pptx" | "xlsx" => {
            // Binary office docs — use structure extraction directly
            let structure = extract_structure(&file.absolute_path, "", ext);
            if ext == "pptx" {
                entry.push_str(&format!("{} [{} slides]\n", file.relative_path, structure.len()));
            } else {
                entry.push_str(&format!("{}\n", file.relative_path));
            }
            for s in &structure {
                entry.push_str(&format!("  {}\n", s.label));
            }
        }
        "pdf" => {
            // PDF — try to get text for line count
            match hive_workspace_index::extract_text(&file.absolute_path) {
                Ok(Some(text)) => {
                    let line_count = text.lines().count();
                    entry.push_str(&format!("{} [{} lines]\n", file.relative_path, line_count));
                }
                _ => {
                    entry.push_str(&format!("{}\n", file.relative_path));
                }
            }
        }
        _ => {
            // Text files — read content, count lines, extract structure
            let content = if hive_workspace_index::is_text_extension(ext) {
                std::fs::read_to_string(&file.absolute_path).ok()
            } else {
                // Try extract_text for other formats
                hive_workspace_index::extract_text(&file.absolute_path).ok().flatten()
            };

            if let Some(ref text) = content {
                let line_count = text.lines().count();
                entry.push_str(&format!("{} [{} lines]\n", file.relative_path, line_count));

                let structure = extract_structure(&file.absolute_path, text, ext);
                for s in &structure {
                    entry.push_str(&format!("  {}: {}\n", s.line, s.label));
                }
            } else {
                // Binary or unreadable — just list the file
                entry.push_str(&format!("{}\n", file.relative_path));
            }
        }
    }

    entry
}

/// Code-oriented workspace context optimised for software-engineering tasks.
///
/// Like [`GeneralContextMap`] but filters out non-code files (PDFs, DOCX,
/// images, etc.), keeping only files whose extension passes
/// `hive_workspace_index::is_text_extension`.
pub struct CodeContextMap;

impl ContextMapProvider for CodeContextMap {
    fn build_context(&self, workspace_path: &str) -> String {
        let files = walk_workspace(workspace_path);
        if files.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Workspace Context Map\n\n");
        let mut truncated = false;

        for file in &files {
            let ext = file
                .absolute_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if !hive_workspace_index::is_text_extension(&ext) {
                continue;
            }

            let file_line = build_file_entry(file, &ext);

            if output.len() + file_line.len() > MAX_OUTPUT_BYTES {
                truncated = true;
                break;
            }

            output.push_str(&file_line);
        }

        if truncated {
            output.push_str("\n... (truncated — workspace has more files)\n");
        }

        // If no code files were found, return empty rather than a bare header
        if output == "## Workspace Context Map\n\n" {
            return String::new();
        }

        output
    }
}

// ── Advanced context map ───────────────────────────────────────────────

/// Dependencies required by LLM-backed context map strategies.
pub struct ContextMapDeps {
    pub model_router: Arc<ArcSwap<ModelRouter>>,
    /// Preferred model patterns for the secondary LLM (e.g. `["gpt-4.1-mini"]`).
    /// Falls back to `preferred_models` from the persona, then default routing.
    pub secondary_models: Option<Vec<String>>,
    /// Preferred model patterns from the persona's main model list.  Used as
    /// fallback when `secondary_models` is `None`.
    pub preferred_models: Option<Vec<String>>,
}

/// Time-to-live for a cached context map entry (safety-net expiry).
const CACHE_TTL_SECS: u64 = 30 * 60; // 30 minutes

/// Maximum tool-use loop iterations to prevent runaway costs.
const MAX_TOOL_ITERATIONS: usize = 20;

/// Maximum characters returned by a single file read.
const MAX_FILE_READ_CHARS: usize = 32_000;

/// Maximum total characters across all search results.
const MAX_SEARCH_RESULT_CHARS: usize = 16_000;

struct CacheEntry {
    fingerprint: String,
    context_map: String,
    created_at: Instant,
}

/// LLM-powered semantic architecture map.
///
/// Uses the [`GeneralContextMap`] to produce a structural workspace overview,
/// then feeds that overview to a secondary LLM equipped with `read_file` and
/// `search_files` tools so it can inspect files it deems important.  The LLM
/// synthesises a rich semantic architecture map from the overview plus any
/// file contents it reads.  Results are cached and invalidated when the
/// workspace file listing changes.
pub struct AdvancedContextMap {
    model_router: Arc<ArcSwap<ModelRouter>>,
    secondary_models: Option<Vec<String>>,
    preferred_models: Option<Vec<String>>,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl AdvancedContextMap {
    pub fn new(deps: ContextMapDeps) -> Self {
        Self {
            model_router: deps.model_router,
            secondary_models: deps.secondary_models,
            preferred_models: deps.preferred_models,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Compute a SHA-256 fingerprint of the workspace file listing.
    fn workspace_fingerprint(files: &[FileEntry]) -> String {
        let mut hasher = Sha256::new();
        for f in files {
            hasher.update(f.relative_path.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    }

    // ── Tool definitions ───────────────────────────────────────────────

    fn tool_definitions() -> Vec<ToolDefinition> {
        let annotations = ToolAnnotations {
            title: String::new(),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        };
        vec![
            ToolDefinition {
                id: "read_file".to_string(),
                name: "read_file".to_string(),
                description: "Read the contents of a file in the workspace. \
                    Returns the file content (truncated to ~32 000 chars if very large)."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file within the workspace (e.g. \"src/main.rs\")."
                        }
                    },
                    "required": ["path"]
                }),
                output_schema: None,
                channel_class: ChannelClass::LocalOnly,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: annotations.clone(),
            },
            ToolDefinition {
                id: "search_files".to_string(),
                name: "search_files".to_string(),
                description: "Search for a regex pattern across all text files in the workspace. \
                    Returns matching lines with file paths and line numbers."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern to search for."
                        },
                        "glob": {
                            "type": "string",
                            "description": "Optional glob to restrict which files to search (e.g. \"*.rs\", \"src/**/*.ts\"). Defaults to all files."
                        }
                    },
                    "required": ["pattern"]
                }),
                output_schema: None,
                channel_class: ChannelClass::LocalOnly,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations,
            },
        ]
    }

    // ── Tool execution ─────────────────────────────────────────────────

    /// Execute a tool call locally, scoped to the workspace.
    fn execute_tool(workspace: &Path, name: &str, args: &serde_json::Value) -> String {
        match name {
            "read_file" => Self::exec_read_file(workspace, args),
            "search_files" => Self::exec_search_files(workspace, args),
            other => format!("Unknown tool: {other}"),
        }
    }

    fn exec_read_file(workspace: &Path, args: &serde_json::Value) -> String {
        let rel_path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: missing required parameter 'path'".to_string(),
        };

        // Normalise and prevent path traversal
        let normalised = rel_path.replace('\\', "/");
        if normalised.contains("..") {
            return "Error: path traversal not allowed".to_string();
        }

        let abs = workspace.join(&normalised);
        if !abs.starts_with(workspace) {
            return "Error: path outside workspace".to_string();
        }

        // Try text extraction first (handles PDFs, Office docs, etc.)
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

        let content = if hive_workspace_index::is_text_extension(&ext) {
            std::fs::read_to_string(&abs).ok()
        } else {
            hive_workspace_index::extract_text(&abs).ok().flatten()
        };

        match content {
            Some(text) => {
                if text.len() > MAX_FILE_READ_CHARS {
                    let truncated: String = text.chars().take(MAX_FILE_READ_CHARS).collect();
                    format!("{truncated}\n\n... (truncated at {MAX_FILE_READ_CHARS} chars)")
                } else {
                    text
                }
            }
            None => format!("Error: could not read file '{rel_path}'"),
        }
    }

    fn exec_search_files(workspace: &Path, args: &serde_json::Value) -> String {
        let pattern_str = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return "Error: missing required parameter 'pattern'".to_string(),
        };

        let re = match regex::Regex::new(pattern_str) {
            Ok(r) => r,
            Err(e) => return format!("Error: invalid regex: {e}"),
        };

        let glob_filter = args.get("glob").and_then(|v| v.as_str());
        let glob_pattern = glob_filter.and_then(|g| glob::Pattern::new(g).ok());

        let files = walk_workspace(workspace.to_str().unwrap_or("."));
        let mut output = String::new();
        let mut total_chars = 0usize;

        for file in &files {
            // Apply glob filter
            if let Some(ref pat) = glob_pattern {
                if !pat.matches(&file.relative_path) {
                    continue;
                }
            }

            let ext = file
                .absolute_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if !hive_workspace_index::is_text_extension(&ext) {
                continue;
            }

            let content = match std::fs::read_to_string(&file.absolute_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (line_no, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    let entry = format!("{}:{}: {}\n", file.relative_path, line_no + 1, line);
                    total_chars += entry.len();
                    output.push_str(&entry);

                    if total_chars >= MAX_SEARCH_RESULT_CHARS {
                        output.push_str("... (results truncated)\n");
                        return output;
                    }
                }
            }
        }

        if output.is_empty() {
            format!("No matches found for pattern '{pattern_str}'")
        } else {
            output
        }
    }

    // ── Tool-use loop ──────────────────────────────────────────────────

    /// Run the LLM with tools, looping until it stops calling tools or we
    /// hit the iteration limit.  Returns the final text response.
    fn tool_loop(&self, workspace_path: &str, initial_prompt: &str) -> Result<String, String> {
        let models = self.secondary_models.clone().or_else(|| self.preferred_models.clone());
        let tools = Self::tool_definitions();
        let workspace = PathBuf::from(workspace_path);

        let mut prompt = initial_prompt.to_string();
        let mut messages: Vec<CompletionMessage> = vec![];

        for iteration in 0..MAX_TOOL_ITERATIONS {
            let request = CompletionRequest {
                prompt: prompt.clone(),
                prompt_content_parts: vec![],
                messages: messages.clone(),
                required_capabilities: BTreeSet::new(),
                preferred_models: models.clone(),
                tools: tools.clone(),
            };

            let router = self.model_router.load();
            let response = router
                .complete(&request)
                .map_err(|e: ModelRouterError| format!("LLM call failed: {e}"))?;

            if response.tool_calls.is_empty() {
                // LLM is done — return its final text
                return Ok(response.content);
            }

            debug!(
                iteration,
                tool_call_count = response.tool_calls.len(),
                "advanced context map: processing tool calls"
            );

            // Append the assistant's response (with tool calls) to messages
            // so the LLM sees the full conversation on the next turn.
            messages.push(CompletionMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                content_parts: vec![],
                blocks: vec![],
            });

            // Execute each tool call and format results
            let mut tool_results = String::new();
            for tc in &response.tool_calls {
                let result = Self::execute_tool(&workspace, &tc.name, &tc.arguments);
                debug!(tool = %tc.name, result_len = result.len(), "tool executed");
                let safe_result = hive_contracts::prompt_sanitize::escape_prompt_tags(&result);
                tool_results.push_str(&format!(
                    "\n\n<tool_call>\n{{\"tool\": \"{}\", \"input\": {}}}\n</tool_call>\n\
                     <tool_result>\n{}\n</tool_result>",
                    tc.name, tc.arguments, safe_result
                ));
            }

            // Append tool results and continue the loop
            prompt = format!("{prompt}{tool_results}");
        }

        warn!("advanced context map: hit tool iteration limit ({MAX_TOOL_ITERATIONS})");
        Err("tool iteration limit reached".to_string())
    }

    // ── Main pipeline ──────────────────────────────────────────────────

    /// Run the General strategy, then hand the LLM the overview + tools
    /// to investigate the codebase and produce a semantic architecture map.
    fn run_pipeline(&self, workspace_path: &str) -> Result<String, String> {
        let general_output = GeneralContextMap.build_context(workspace_path);
        if general_output.is_empty() {
            return Ok(String::new());
        }

        let files = walk_workspace(workspace_path);
        let ws_type = classify_workspace(&files);

        info!(
            general_len = general_output.len(),
            workspace_type = ?ws_type,
            "Advanced context map: starting tool-assisted synthesis"
        );

        let prompt = match ws_type {
            WorkspaceType::Code | WorkspaceType::Mixed => format!(
                "You are analysing a software project to produce an architecture overview \
                 that will be used as context for a coding assistant.\n\n\
                 Below is a structural overview of the workspace listing every file with \
                 its key definitions (functions, structs, classes, headings, etc.).\n\n\
                 <workspace_structure>\n{general_output}\n</workspace_structure>\n\n\
                 You have tools to read individual files and search the codebase. Use \
                 them to inspect key files you think are important for understanding \
                 the architecture — for example entry points, configuration files, \
                 main modules, READMEs, Cargo.toml / package.json, and core type \
                 definitions.\n\n\
                 After investigating, synthesise a comprehensive architecture overview \
                 covering:\n\
                 - Overall project purpose and architecture style\n\
                 - Key modules / packages and their responsibilities\n\
                 - Module dependency graph and data flow\n\
                 - Key abstractions, traits, interfaces, and extension points\n\
                 - Entry points and public APIs\n\
                 - Cross-cutting concerns (error handling, logging, auth, etc.)\n\
                 - Notable patterns or architectural decisions\n\n\
                 Produce a well-structured architecture map in 800-1500 words \
                 formatted as Markdown. Focus on giving a future reader (or AI \
                 assistant) a clear mental model of how the codebase is organised \
                 and how the pieces fit together."
            ),
            WorkspaceType::Documents => format!(
                "You are analysing a document collection to produce an overview that \
                 will be used as context for an AI assistant.\n\n\
                 Below is a structural overview of the workspace listing every file \
                 with its key sections and headings.\n\n\
                 <workspace_structure>\n{general_output}\n</workspace_structure>\n\n\
                 You have tools to read individual files and search across documents. \
                 Use them to inspect key files you think are important for understanding \
                 the collection — for example README files, index documents, \
                 table-of-contents files, and representative documents from each \
                 category.\n\n\
                 After investigating, synthesise a comprehensive overview covering:\n\
                 - Collection purpose and scope\n\
                 - Document categories and organization\n\
                 - Key themes and topics across documents\n\
                 - Important documents and their relationships\n\
                 - Naming conventions and file organization patterns\n\
                 - Any metadata, indices, or cross-references\n\n\
                 Produce a well-structured overview in 800-1500 words formatted as \
                 Markdown. Focus on giving a future reader (or AI assistant) a clear \
                 understanding of what this document collection contains and how it \
                 is organised."
            ),
        };
        self.tool_loop(workspace_path, &prompt)
    }
}

impl ContextMapProvider for AdvancedContextMap {
    fn build_context(&self, workspace_path: &str) -> String {
        let files = walk_workspace(workspace_path);
        if files.is_empty() {
            return String::new();
        }

        let fingerprint = Self::workspace_fingerprint(&files);

        // Check cache
        {
            let cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.get(workspace_path) {
                let age = entry.created_at.elapsed().as_secs();
                if entry.fingerprint == fingerprint && age < CACHE_TTL_SECS {
                    debug!(workspace = workspace_path, age_secs = age, "cache hit");
                    return entry.context_map.clone();
                }
            }
        }

        // Cache miss → run pipeline
        info!(workspace = workspace_path, "cache miss — running advanced pipeline");
        match self.run_pipeline(workspace_path) {
            Ok(context_map) => {
                // Store in cache
                let mut cache = self.cache.lock().unwrap();
                cache.insert(
                    workspace_path.to_string(),
                    CacheEntry {
                        fingerprint,
                        context_map: context_map.clone(),
                        created_at: Instant::now(),
                    },
                );
                context_map
            }
            Err(e) => {
                warn!(error = %e, "advanced pipeline failed, falling back to General");
                // Try stale cache first
                {
                    let cache = self.cache.lock().unwrap();
                    if let Some(entry) = cache.get(workspace_path) {
                        warn!("returning stale cached context map");
                        return entry.context_map.clone();
                    }
                }
                // Final fallback: General strategy
                GeneralContextMap.build_context(workspace_path)
            }
        }
    }
}

/// Return the appropriate [`ContextMapProvider`] for the given strategy.
///
/// For [`ContextMapStrategy::Advanced`] the caller must supply
/// [`ContextMapDeps`]; if `deps` is `None` the strategy silently degrades
/// to [`ContextMapStrategy::General`].
pub fn context_map_for(
    strategy: &ContextMapStrategy,
    deps: Option<ContextMapDeps>,
) -> Box<dyn ContextMapProvider> {
    match strategy {
        ContextMapStrategy::General => Box::new(GeneralContextMap),
        ContextMapStrategy::Code => Box::new(CodeContextMap),
        ContextMapStrategy::Advanced => {
            match deps {
                Some(d) => Box::new(AdvancedContextMap::new(d)),
                None => {
                    warn!("Advanced context map requested but no deps provided; falling back to General");
                    Box::new(GeneralContextMap)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_builds_context_for_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("README.md"),
            "# My Project\n\nSome description.\n\n## Installation\n\nRun it.\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n\nstruct Config {\n    name: String,\n}\n",
        )
        .unwrap();

        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        assert!(result.contains("## Workspace Context Map"));
        assert!(result.contains("README.md"));
        assert!(result.contains("# My Project"));
        assert!(result.contains("## Installation"));
        assert!(result.contains("main.rs"));
        assert!(result.contains("fn main"));
        assert!(result.contains("struct Config"));
    }

    #[test]
    fn general_ignores_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let git = tmp.path().join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main").unwrap();
        std::fs::write(tmp.path().join("src.rs"), "fn hello() {}").unwrap();

        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        assert!(!result.contains(".git"));
        assert!(!result.contains("HEAD"));
        assert!(result.contains("src.rs"));
    }

    #[test]
    fn general_empty_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());
        assert_eq!(result, "");
    }

    #[test]
    fn general_truncates_large_output() {
        let tmp = tempfile::tempdir().unwrap();
        // Create many files to exceed MAX_OUTPUT_BYTES
        for i in 0..5000 {
            let content = format!("fn func_{i}() {{}}\n").repeat(20);
            std::fs::write(tmp.path().join(format!("file_{i:04}.rs")), content).unwrap();
        }

        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        assert!(result.len() <= MAX_OUTPUT_BYTES + 200); // small margin for truncation message
        assert!(result.contains("truncated"));
    }

    #[test]
    fn code_builds_context_for_code_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(tmp.path().join("lib.py"), "def hello(): pass\n").unwrap();
        // A non-text file that should be excluded
        std::fs::write(tmp.path().join("photo.png"), &[0x89, 0x50, 0x4E, 0x47]).unwrap();

        let provider = CodeContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        assert!(result.contains("## Workspace Context Map"));
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.py"));
        assert!(!result.contains("photo.png"));
    }

    #[test]
    fn code_returns_empty_for_only_binary_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("photo.png"), &[0x89, 0x50]).unwrap();
        std::fs::write(tmp.path().join("data.bin"), &[0x00, 0xFF]).unwrap();

        let provider = CodeContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());
        assert_eq!(result, "");
    }

    #[test]
    fn code_returns_empty_for_empty_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = CodeContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());
        assert_eq!(result, "");
    }

    #[test]
    fn factory_returns_general_for_general_strategy() {
        let provider = context_map_for(&ContextMapStrategy::General, None);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("test.txt"), "hello").unwrap();
        let result = provider.build_context(tmp.path().to_str().unwrap());
        assert!(result.contains("test.txt"));
    }

    #[test]
    fn factory_returns_code_for_code_strategy() {
        let provider = context_map_for(&ContextMapStrategy::Code, None);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
        let result = provider.build_context(tmp.path().to_str().unwrap());
        assert!(result.contains("main.rs"));
    }

    #[test]
    fn factory_advanced_without_deps_falls_back_to_general() {
        let provider = context_map_for(&ContextMapStrategy::Advanced, None);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("test.txt"), "hello world").unwrap();
        let result = provider.build_context(tmp.path().to_str().unwrap());
        // Should produce General output since no deps were provided
        assert!(result.contains("test.txt"));
    }

    #[test]
    fn context_map_strategy_serde_round_trip() {
        let general: ContextMapStrategy =
            serde_json::from_str(r#""general""#).expect("deser general");
        assert_eq!(general, ContextMapStrategy::General);

        let code: ContextMapStrategy = serde_json::from_str(r#""code""#).expect("deser code");
        assert_eq!(code, ContextMapStrategy::Code);

        let advanced: ContextMapStrategy =
            serde_json::from_str(r#""advanced""#).expect("deser advanced");
        assert_eq!(advanced, ContextMapStrategy::Advanced);

        assert_eq!(serde_json::to_string(&ContextMapStrategy::General).unwrap(), r#""general""#);
        assert_eq!(serde_json::to_string(&ContextMapStrategy::Code).unwrap(), r#""code""#);
        assert_eq!(serde_json::to_string(&ContextMapStrategy::Advanced).unwrap(), r#""advanced""#);
    }

    #[test]
    fn context_map_strategy_default_is_general() {
        assert_eq!(ContextMapStrategy::default(), ContextMapStrategy::General);
    }

    #[test]
    fn csv_file_shows_columns() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("data.csv"), "Name,Email,Dept\nAlice,a@b.com,Eng\n")
            .unwrap();

        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        assert!(result.contains("data.csv"));
        assert!(result.contains("Columns: Name, Email, Dept"));
    }

    #[test]
    fn nested_workspace_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "pub mod utils;\n\npub struct App {}\n").unwrap();
        std::fs::write(tmp.path().join("README.md"), "# Project\n\n## Usage\n\nSee docs.\n")
            .unwrap();

        let provider = GeneralContextMap;
        let result = provider.build_context(tmp.path().to_str().unwrap());

        // Verify ordering: README.md before src/lib.rs (alphabetical)
        let readme_pos = result.find("README.md").unwrap();
        let lib_pos = result.find("src/lib.rs").unwrap();
        assert!(readme_pos < lib_pos);

        assert!(result.contains("# Project"));
        assert!(result.contains("## Usage"));
        assert!(result.contains("mod utils"));
        assert!(result.contains("struct App"));
    }

    #[test]
    fn workspace_fingerprint_changes_with_files() {
        let files_a = vec![FileEntry {
            relative_path: "a.rs".to_string(),
            absolute_path: std::path::PathBuf::from("/w/a.rs"),
        }];
        let files_b = vec![
            FileEntry {
                relative_path: "a.rs".to_string(),
                absolute_path: std::path::PathBuf::from("/w/a.rs"),
            },
            FileEntry {
                relative_path: "b.rs".to_string(),
                absolute_path: std::path::PathBuf::from("/w/b.rs"),
            },
        ];

        let fp_a = AdvancedContextMap::workspace_fingerprint(&files_a);
        let fp_b = AdvancedContextMap::workspace_fingerprint(&files_b);
        assert_ne!(fp_a, fp_b);

        // Same files → same fingerprint
        let fp_a2 = AdvancedContextMap::workspace_fingerprint(&files_a);
        assert_eq!(fp_a, fp_a2);
    }

    #[test]
    fn advanced_empty_workspace_returns_empty() {
        let router = Arc::new(ArcSwap::from(Arc::new(ModelRouter::new())));
        let provider = AdvancedContextMap::new(ContextMapDeps {
            model_router: router,
            secondary_models: None,
            preferred_models: None,
        });
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(provider.build_context(tmp.path().to_str().unwrap()), "");
    }

    #[test]
    fn advanced_falls_back_to_general_on_no_providers() {
        // ModelRouter with no providers → all LLM calls fail → should
        // fall back to GeneralContextMap output.
        let router = Arc::new(ArcSwap::from(Arc::new(ModelRouter::new())));
        let provider = AdvancedContextMap::new(ContextMapDeps {
            model_router: router,
            secondary_models: None,
            preferred_models: None,
        });
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("hello.rs"), "fn main() {}").unwrap();
        let result = provider.build_context(tmp.path().to_str().unwrap());
        // Should contain General-style output as fallback
        assert!(result.contains("hello.rs"));
    }

    #[test]
    fn tool_read_file_returns_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("foo.rs"), "fn hello() { 42 }").unwrap();

        let args = serde_json::json!({"path": "foo.rs"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "read_file", &args);
        assert!(result.contains("fn hello()"));
    }

    #[test]
    fn tool_read_file_blocks_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "../../../etc/passwd"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "read_file", &args);
        assert!(result.contains("Error"));
    }

    #[test]
    fn tool_read_file_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "nonexistent.rs"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "read_file", &args);
        assert!(result.contains("Error"));
    }

    #[test]
    fn tool_search_files_finds_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn gamma() {}\nfn alpha_2() {}\n").unwrap();

        let args = serde_json::json!({"pattern": "alpha"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "search_files", &args);
        assert!(result.contains("a.rs:1:"));
        assert!(result.contains("b.rs:2:"));
    }

    #[test]
    fn tool_search_files_with_glob_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn target() {}").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "target here too").unwrap();

        let args = serde_json::json!({"pattern": "target", "glob": "*.rs"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "search_files", &args);
        assert!(result.contains("a.rs"));
        assert!(!result.contains("b.txt"));
    }

    #[test]
    fn tool_search_files_no_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn hello() {}").unwrap();

        let args = serde_json::json!({"pattern": "zzzznotfound"});
        let result = AdvancedContextMap::execute_tool(tmp.path(), "search_files", &args);
        assert!(result.contains("No matches"));
    }

    #[test]
    fn tool_definitions_are_valid() {
        let tools = AdvancedContextMap::tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].id, "read_file");
        assert_eq!(tools[1].id, "search_files");
        // Verify schemas parse
        for t in &tools {
            assert!(t.input_schema.get("properties").is_some());
        }
    }

    // ── classify_workspace tests ───────────────────────────────────────

    fn make_file_entry(name: &str) -> FileEntry {
        FileEntry {
            relative_path: name.to_string(),
            absolute_path: PathBuf::from(format!("/w/{name}")),
        }
    }

    #[test]
    fn classify_all_code_files() {
        let files =
            vec![make_file_entry("main.rs"), make_file_entry("lib.py"), make_file_entry("app.tsx")];
        assert_eq!(classify_workspace(&files), WorkspaceType::Code);
    }

    #[test]
    fn classify_all_document_files() {
        let files = vec![
            make_file_entry("report.pdf"),
            make_file_entry("notes.docx"),
            make_file_entry("slides.pptx"),
        ];
        assert_eq!(classify_workspace(&files), WorkspaceType::Documents);
    }

    #[test]
    fn classify_mixed_workspace() {
        let files = vec![
            make_file_entry("main.rs"),
            make_file_entry("report.pdf"),
            make_file_entry("photo.png"),
        ];
        assert_eq!(classify_workspace(&files), WorkspaceType::Mixed);
    }

    #[test]
    fn classify_empty_workspace_defaults_to_code() {
        assert_eq!(classify_workspace(&[]), WorkspaceType::Code);
    }

    #[test]
    fn classify_majority_code_with_some_docs() {
        let files = vec![
            make_file_entry("a.rs"),
            make_file_entry("b.rs"),
            make_file_entry("c.rs"),
            make_file_entry("readme.pdf"),
        ];
        assert_eq!(classify_workspace(&files), WorkspaceType::Code);
    }

    #[test]
    fn classify_majority_docs_with_some_code() {
        let files = vec![
            make_file_entry("a.pdf"),
            make_file_entry("b.docx"),
            make_file_entry("c.xlsx"),
            make_file_entry("script.py"),
        ];
        assert_eq!(classify_workspace(&files), WorkspaceType::Documents);
    }
}
