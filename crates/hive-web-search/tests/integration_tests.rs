use std::sync::Arc;

use hive_contracts::WebSearchConfig;
use hive_model::ModelRouter;
use hive_tools::Tool;
use hive_web_search::{ContentExtractor, SearchResult, WebSearchTool};

// ─── 1. Config resolution tests ────────────────────────────────────────────

#[test]
fn resolve_api_key_literal_value() {
    let config =
        WebSearchConfig { provider: "brave".into(), api_key: Some("sk-literal-key-12345".into()) };
    assert_eq!(config.resolve_api_key(), Some("sk-literal-key-12345".into()));
}

#[test]
fn resolve_api_key_from_env_var() {
    // Use a unique var name to avoid collisions with parallel tests.
    let var_name = "HIVEMIND_TEST_WS_KEY_1";
    std::env::set_var(var_name, "secret-from-env");

    let config =
        WebSearchConfig { provider: "brave".into(), api_key: Some(format!("env:{var_name}")) };
    assert_eq!(config.resolve_api_key(), Some("secret-from-env".into()));

    std::env::remove_var(var_name);
}

#[test]
fn resolve_api_key_missing_env_var_returns_none() {
    // Refer to a variable that definitely does not exist.
    let config = WebSearchConfig {
        provider: "brave".into(),
        api_key: Some("env:HIVEMIND_TEST_NONEXISTENT_VAR_XYZ".into()),
    };
    assert_eq!(config.resolve_api_key(), None);
}

#[test]
fn resolve_api_key_none_returns_none() {
    let config = WebSearchConfig { provider: "brave".into(), api_key: None };
    assert_eq!(config.resolve_api_key(), None);
}

// ─── 2. Backend selection via from_config ───────────────────────────────────

fn make_router() -> Arc<ModelRouter> {
    Arc::new(ModelRouter::new())
}

#[test]
fn from_config_brave_with_key_returns_some() {
    let config = WebSearchConfig { provider: "brave".into(), api_key: Some("test-key".into()) };
    let tool = WebSearchTool::from_config(&config, make_router(), None);
    assert!(tool.is_some());
    let tool = tool.unwrap();
    let def = tool.definition();
    assert_eq!(def.id, "web.search");
    assert_eq!(def.name, "Web Search");
    assert!(!def.description.is_empty());
}

#[test]
fn from_config_tavily_with_key_returns_some() {
    let config = WebSearchConfig { provider: "tavily".into(), api_key: Some("test-key".into()) };
    let tool = WebSearchTool::from_config(&config, make_router(), None);
    assert!(tool.is_some());
    let tool = tool.unwrap();
    let def = tool.definition();
    assert_eq!(def.id, "web.search");
    assert_eq!(def.name, "Web Search");
    assert!(!def.description.is_empty());
}

#[test]
fn from_config_provider_none_returns_none() {
    let config = WebSearchConfig { provider: "none".into(), api_key: Some("test-key".into()) };
    assert!(WebSearchTool::from_config(&config, make_router(), None).is_none());
}

#[test]
fn from_config_unknown_provider_returns_none() {
    let config =
        WebSearchConfig { provider: "duckduckgo".into(), api_key: Some("test-key".into()) };
    assert!(WebSearchTool::from_config(&config, make_router(), None).is_none());
}

#[test]
fn from_config_missing_api_key_returns_none() {
    let config = WebSearchConfig { provider: "brave".into(), api_key: None };
    assert!(WebSearchTool::from_config(&config, make_router(), None).is_none());
}

#[test]
fn from_config_empty_provider_returns_none() {
    let config = WebSearchConfig { provider: "".into(), api_key: Some("test-key".into()) };
    assert!(WebSearchTool::from_config(&config, make_router(), None).is_none());
}

// ─── 3. Content extraction with pre-populated content ──────────────────────

#[tokio::test]
async fn extraction_uses_preextracted_content() {
    let extractor = ContentExtractor::default();
    let results = vec![SearchResult {
        title: "Test".into(),
        url: "https://example.com".into(),
        snippet: "snippet".into(),
        content: Some("Pre-extracted content from Tavily".into()),
    }];

    let pages = extractor.extract_content(&results).await;
    assert_eq!(pages.len(), 1);
    assert!(pages[0].content.contains("Pre-extracted content from Tavily"));
}

#[tokio::test]
async fn extraction_falls_back_to_snippet_on_failed_fetch() {
    let extractor = ContentExtractor::default();
    let results = vec![SearchResult {
        title: "Unreachable".into(),
        url: "http://invalid.test.example.invalid/nope".into(),
        snippet: "fallback snippet text".into(),
        content: None,
    }];

    let pages = extractor.extract_content(&results).await;
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].content, "fallback snippet text");
}

#[tokio::test]
async fn extraction_skips_empty_preextracted_content() {
    let extractor = ContentExtractor::default();
    let results = vec![SearchResult {
        title: "Empty content".into(),
        url: "http://invalid.test.example.invalid/nope".into(),
        snippet: "snippet used as fallback".into(),
        content: Some("".into()),
    }];

    // Empty `content` should not be used — extractor will try to fetch, fail, and
    // fall back to the snippet.
    let pages = extractor.extract_content(&results).await;
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].content, "snippet used as fallback");
}

#[tokio::test]
async fn extraction_preserves_metadata() {
    let extractor = ContentExtractor::default();
    let results = vec![SearchResult {
        title: "My Title".into(),
        url: "https://example.com/page".into(),
        snippet: "My snippet".into(),
        content: Some("Full body text".into()),
    }];

    let pages = extractor.extract_content(&results).await;
    assert_eq!(pages[0].title, "My Title");
    assert_eq!(pages[0].url, "https://example.com/page");
    assert_eq!(pages[0].snippet, "My snippet");
}

// ─── 4. Whitespace-only pre-extracted content falls back to fetch ────────────

#[tokio::test]
async fn extraction_skips_whitespace_only_preextracted_content() {
    let extractor = ContentExtractor::default();
    let results = vec![SearchResult {
        title: "Test".into(),
        url: "https://invalid.test.example".into(), // will fail fetch
        snippet: "fallback snippet".into(),
        content: Some("   \n\t  ".into()), // whitespace only
    }];
    let pages = extractor.extract_content(&results).await;
    assert_eq!(pages.len(), 1);
    // Should have fallen through to fetch (which fails), then to snippet
    assert_eq!(pages[0].content, "fallback snippet");
}
