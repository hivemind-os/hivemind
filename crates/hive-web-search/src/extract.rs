use reqwest::Client;
use scraper::{Html, Selector};
use tracing::warn;

use crate::backend::SearchResult;

/// Fetches and extracts main text content from web pages.
pub struct ContentExtractor {
    client: Client,
    max_content_chars: usize,
}

impl ContentExtractor {
    pub fn new(max_content_chars: usize) -> Self {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (compatible; HiveMindAgent/1.0)")
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build HTTP client");
        Self { client, max_content_chars }
    }

    /// Fetch and extract text from a list of search results.
    /// Returns the results enriched with extracted content.
    pub async fn extract_content(&self, results: &[SearchResult]) -> Vec<ExtractedPage> {
        let mut pages = Vec::with_capacity(results.len());

        for result in results {
            // If the backend already provided extracted content, use it directly.
            if let Some(text) = &result.content {
                if !text.trim().is_empty() {
                    pages.push(ExtractedPage {
                        title: result.title.clone(),
                        url: result.url.clone(),
                        snippet: result.snippet.clone(),
                        content: text.clone(),
                    });
                    continue;
                }
            }

            match self.fetch_and_extract(&result.url).await {
                Ok(content) => {
                    let content =
                        if content.trim().is_empty() { result.snippet.clone() } else { content };
                    pages.push(ExtractedPage {
                        title: result.title.clone(),
                        url: result.url.clone(),
                        snippet: result.snippet.clone(),
                        content,
                    });
                }
                Err(e) => {
                    warn!(url = %result.url, error = %e, "failed to extract content");
                    pages.push(ExtractedPage {
                        title: result.title.clone(),
                        url: result.url.clone(),
                        snippet: result.snippet.clone(),
                        content: result.snippet.clone(),
                    });
                }
            }
        }

        pages
    }

    async fn fetch_and_extract(&self, url: &str) -> Result<String, reqwest::Error> {
        let resp = self.client.get(url).send().await?;
        let resp = resp.error_for_status()?;
        let html = resp.text().await?;
        Ok(extract_main_content(&html, self.max_content_chars))
    }
}

impl Default for ContentExtractor {
    fn default() -> Self {
        Self::new(4000)
    }
}

/// A search result with extracted page content.
#[derive(Debug, Clone)]
pub struct ExtractedPage {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub content: String,
}

/// Extract main text content from HTML, stripping boilerplate.
fn extract_main_content(html: &str, max_chars: usize) -> String {
    let document = Html::parse_document(html);

    // Try to find main content areas
    let content_selectors = [
        "article",
        "main",
        "[role=\"main\"]",
        ".post-content",
        ".article-content",
        ".entry-content",
        ".content",
        "#content",
        ".markdown-body",
        ".readme",
    ];

    for sel_str in &content_selectors {
        if let Ok(selector) = Selector::parse(sel_str) {
            if let Some(el) = document.select(&selector).next() {
                let text = collect_text_content(&el);
                if text.len() > 100 {
                    return truncate_to_chars(&text, max_chars);
                }
            }
        }
    }

    // Fallback: extract from body, skipping nav/header/footer/script/style
    if let Ok(body_sel) = Selector::parse("body") {
        if let Some(body) = document.select(&body_sel).next() {
            let text = collect_text_content(&body);
            return truncate_to_chars(&text, max_chars);
        }
    }

    // Last resort: all text
    let text: String = document.root_element().text().collect::<Vec<_>>().join(" ");
    truncate_to_chars(&clean_whitespace(&text), max_chars)
}

fn collect_text_content(element: &scraper::ElementRef<'_>) -> String {
    let skip_tags: std::collections::HashSet<&str> =
        ["script", "style", "nav", "header", "footer", "aside", "noscript", "iframe"]
            .into_iter()
            .collect();

    let mut parts = Vec::new();
    let mut skip_depth: usize = 0;

    for edge in element.traverse() {
        match edge {
            ego_tree::iter::Edge::Open(node) => {
                if let Some(el) = node.value().as_element() {
                    if skip_tags.contains(el.name()) {
                        skip_depth += 1;
                    }
                }
                if skip_depth == 0 {
                    if let Some(text) = node.value().as_text() {
                        let t = text.trim();
                        if !t.is_empty() {
                            parts.push(t.to_string());
                        }
                    }
                }
            }
            ego_tree::iter::Edge::Close(node) => {
                if let Some(el) = node.value().as_element() {
                    if skip_tags.contains(el.name()) && skip_depth > 0 {
                        skip_depth -= 1;
                    }
                }
            }
        }
    }

    clean_whitespace(&parts.join(" "))
}

fn clean_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut last_was_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }
    result.trim().to_string()
}

fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    // Use char_indices to safely handle multi-byte UTF-8
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    // Find the byte offset of the max_chars-th character
    let byte_offset = s.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(s.len());
    // Find a word boundary near that offset
    let boundary = s[..byte_offset].rfind(char::is_whitespace).unwrap_or(byte_offset);
    format!("{}...", &s[..boundary])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_main_content_article() {
        let html = r#"
        <html><body>
            <nav>Skip me</nav>
            <article><p>This is the main article content that should be extracted.</p></article>
            <footer>Footer junk</footer>
        </body></html>"#;
        let content = extract_main_content(html, 1000);
        assert!(content.contains("main article content"));
        assert!(!content.contains("Skip me"));
    }

    #[test]
    fn test_clean_whitespace() {
        assert_eq!(clean_whitespace("  hello   world  \n\t foo  "), "hello world foo");
    }

    #[test]
    fn test_truncate() {
        let long = "word ".repeat(100);
        let truncated = truncate_to_chars(&long, 20);
        assert!(truncated.ends_with("..."));
        // Should be approximately 20 chars (finding word boundary) + "..."
        assert!(truncated.chars().count() <= 24);
    }

    #[test]
    fn test_truncate_multibyte_utf8() {
        // Each emoji is 4 bytes; truncating by char count should not panic
        let emojis = "🔬🧪🔍🧬🧫🔭📡💡🛠️⚙️";
        let truncated = truncate_to_chars(emojis, 3);
        assert!(truncated.ends_with("..."));
        // Should not panic on multi-byte boundaries
    }

    #[tokio::test]
    async fn test_extract_skips_fetch_when_content_present() {
        let extractor = ContentExtractor::new(4000);
        let results = vec![
            SearchResult {
                title: "Pre-extracted".into(),
                url: "http://invalid.example.com/should-not-fetch".into(),
                snippet: "short snippet".into(),
                content: Some("Full pre-extracted content from the backend API.".into()),
            },
            SearchResult {
                title: "Empty content falls back".into(),
                url: "http://invalid.example.com/also-should-not-resolve".into(),
                snippet: "fallback snippet".into(),
                content: None,
            },
        ];

        let pages = extractor.extract_content(&results).await;
        assert_eq!(pages.len(), 2);

        // First result: should use pre-extracted content verbatim (no fetch).
        assert_eq!(pages[0].content, "Full pre-extracted content from the backend API.");

        // Second result: fetch will fail (invalid host), so it falls back to the snippet.
        assert_eq!(pages[1].content, "fallback snippet");
    }
}
