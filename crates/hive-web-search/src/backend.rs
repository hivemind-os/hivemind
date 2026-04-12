use reqwest::Client;
use serde::{Deserialize, Serialize};

/// A single web search result (URL + title + snippet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Pre-extracted content from the search backend; when present the content
    /// extraction pipeline skips fetching the page over HTTP.
    pub content: Option<String>,
}

/// Trait for pluggable search backends.
#[async_trait::async_trait]
pub trait SearchBackend: Send + Sync {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("failed to parse search results: {0}")]
    Parse(String),
    #[error("search API error: {status} — {message}")]
    Api { status: u16, message: String },
}

// ─── Brave Search ────────────────────────────────────────────────────────────

/// Brave Search API backend.
/// Docs: <https://api.search.brave.com/app/documentation/web-search/get-started>
pub struct BraveBackend {
    client: Client,
    api_key: String,
}

impl BraveBackend {
    pub fn new(api_key: String) -> Result<Self, reqwest::Error> {
        let client = Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
        Ok(Self { client, api_key })
    }
}

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}
#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}
#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[async_trait::async_trait]
impl SearchBackend for BraveBackend {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let resp = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &max_results.min(20).to_string())])
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SearchError::Api { status: status.as_u16(), message: body });
        }

        let data: BraveResponse = resp
            .json()
            .await
            .map_err(|e| SearchError::Parse(format!("failed to parse Brave response: {e}")))?;

        let results = data
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
                content: None,
            })
            .collect();

        Ok(results)
    }
}

// ─── Tavily ──────────────────────────────────────────────────────────────────

/// Tavily Search API backend.
/// Docs: <https://docs.tavily.com/>
pub struct TavilyBackend {
    client: Client,
    api_key: String,
}

impl TavilyBackend {
    pub fn new(api_key: String) -> Result<Self, reqwest::Error> {
        let client = Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
        Ok(Self { client, api_key })
    }
}

#[derive(Serialize)]
struct TavilyRequest<'a> {
    query: &'a str,
    max_results: usize,
    include_answer: bool,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}
#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait::async_trait]
impl SearchBackend for TavilyBackend {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let body = TavilyRequest { query, max_results: max_results.min(10), include_answer: false };

        let resp = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(SearchError::Api { status: status.as_u16(), message: text });
        }

        let data: TavilyResponse = resp
            .json()
            .await
            .map_err(|e| SearchError::Parse(format!("failed to parse Tavily response: {e}")))?;

        let results = data
            .results
            .into_iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.clone(),
                content: Some(r.content),
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brave_response_parses() {
        let json = r#"{
            "web": {
                "results": [
                    {"title": "Rust", "url": "https://rust-lang.org", "description": "A systems language"}
                ]
            }
        }"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results = resp.web.unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert_eq!(results[0].description.as_deref(), Some("A systems language"));
    }

    #[test]
    fn brave_response_maps_to_search_results() {
        let json = r#"{
            "web": {
                "results": [
                    {"title": "Rust", "url": "https://rust-lang.org", "description": "A systems language"},
                    {"title": "Rust Book", "url": "https://doc.rust-lang.org/book/", "description": "The Rust book"}
                ]
            }
        }"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
                content: None,
            })
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert_eq!(results[0].snippet, "A systems language");
        assert!(results[0].content.is_none());
        assert!(results[1].content.is_none());
    }

    #[test]
    fn brave_response_with_null_web_yields_empty() {
        let json = r#"{ "web": null }"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
                content: None,
            })
            .collect();
        assert!(results.is_empty());
    }

    #[test]
    fn brave_response_with_missing_web_yields_empty() {
        let json = r#"{}"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
                content: None,
            })
            .collect();
        assert!(results.is_empty());
    }

    #[test]
    fn tavily_response_parses() {
        let json = r#"{
            "results": [
                {"title": "Rust", "url": "https://rust-lang.org", "content": "A systems language"}
            ]
        }"#;
        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].title, "Rust");
        assert_eq!(resp.results[0].url, "https://rust-lang.org");
        assert_eq!(resp.results[0].content, "A systems language");
    }

    #[test]
    fn tavily_response_maps_to_search_results_with_content() {
        let json = r#"{
            "results": [
                {"title": "Rust", "url": "https://rust-lang.org", "content": "Rich content from Tavily"}
            ]
        }"#;
        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.clone(),
                content: Some(r.content),
            })
            .collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert_eq!(results[0].snippet, "Rich content from Tavily");
        assert_eq!(results[0].content.as_deref(), Some("Rich content from Tavily"));
    }

    #[test]
    fn tavily_response_with_empty_results() {
        let json = r#"{ "results": [] }"#;
        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert!(resp.results.is_empty());
    }
}
