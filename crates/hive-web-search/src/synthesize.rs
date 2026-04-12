use std::collections::BTreeSet;
use std::sync::Arc;

use hive_contracts::Capability;
use hive_model::{CompletionMessage, CompletionRequest, ModelRouter};

use crate::extract::ExtractedPage;

/// Synthesizes extracted web content into a coherent answer using the model router.
#[derive(Clone)]
pub struct SearchSynthesizer {
    router: Arc<ModelRouter>,
    preferred_models: Option<Vec<String>>,
}

impl SearchSynthesizer {
    pub fn new(router: Arc<ModelRouter>, preferred_models: Option<Vec<String>>) -> Self {
        Self { router, preferred_models }
    }

    /// Synthesize a coherent answer from extracted pages and the original query.
    pub fn synthesize(
        &self,
        query: &str,
        pages: &[ExtractedPage],
    ) -> Result<SynthesisResult, SynthesisError> {
        if pages.is_empty() {
            return Ok(SynthesisResult {
                answer: "No search results found.".to_string(),
                sources: Vec::new(),
            });
        }

        let (context, included_count) = build_context(pages);

        if included_count == 0 || context.trim().is_empty() {
            return Ok(SynthesisResult {
                answer: format!(
                    "Found {} search results but could not extract usable content from any of them.",
                    pages.len()
                ),
                sources: pages
                    .iter()
                    .map(|p| SourceRef {
                        index: 0,
                        title: p.title.clone(),
                        url: p.url.clone(),
                        snippet: p.snippet.clone(),
                    })
                    .collect(),
            });
        }

        let system_prompt = SYNTHESIS_SYSTEM_PROMPT;
        let user_prompt = format!(
            "## Search Query\n{query}\n\n## Search Results\n{context}\n\n\
             Synthesize the above search results into a comprehensive answer to the query. \
             Include inline citations like [1], [2] etc. referring to the source numbers above."
        );

        let request = CompletionRequest {
            prompt: user_prompt.clone(),
            prompt_content_parts: vec![],
            messages: vec![CompletionMessage::text("system", system_prompt)],
            required_capabilities: BTreeSet::from([Capability::Chat]),
            preferred_models: self.preferred_models.clone(),
            tools: vec![],
        };

        let response = self.router.complete(&request)?;

        let sources: Vec<SourceRef> = pages
            .iter()
            .take(included_count)
            .enumerate()
            .map(|(i, p)| SourceRef {
                index: i + 1,
                title: p.title.clone(),
                url: p.url.clone(),
                snippet: p.snippet.clone(),
            })
            .collect();

        Ok(SynthesisResult { answer: response.content, sources })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SynthesisResult {
    pub answer: String,
    pub sources: Vec<SourceRef>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRef {
    pub index: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SynthesisError {
    #[error("model routing failed: {0}")]
    Routing(#[from] hive_model::ModelRouterError),
}

fn build_context(pages: &[ExtractedPage]) -> (String, usize) {
    const MAX_CONTEXT_CHARS: usize = 30_000;
    let mut ctx = String::new();
    let mut included = 0;
    for (i, page) in pages.iter().enumerate() {
        let entry = format!(
            "### Source [{}]: {}\nURL: {}\n\n{}\n\n---\n\n",
            i + 1,
            page.title,
            page.url,
            page.content,
        );
        if ctx.len() + entry.len() > MAX_CONTEXT_CHARS {
            // Include the truncated stub so the model knows this source exists
            // but count it as included since it appears in the context.
            ctx.push_str(&format!(
                "### Source [{}]: {}\nURL: {}\n\n[Content truncated due to size]\n\n---\n\n",
                i + 1,
                page.title,
                page.url,
            ));
            included += 1;
            break;
        }
        ctx.push_str(&entry);
        included += 1;
    }
    (ctx, included)
}

const SYNTHESIS_SYSTEM_PROMPT: &str = "\
You are a search synthesis engine. Your job is to take web search results and \
produce a clear, accurate, and comprehensive answer to the user's question.

Rules:
- Use inline citations like [1], [2] to reference sources
- Be factual and precise — only state what the sources support
- If sources conflict, note the disagreement
- Structure your answer with clear paragraphs
- Include code examples if relevant and present in the sources
- Keep your answer concise but thorough
- If the sources don't adequately answer the question, say so";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_context_caps_size() {
        let big_content = "x".repeat(25_000);
        let pages = vec![
            ExtractedPage {
                url: "https://a.com".into(),
                title: "Page A".into(),
                snippet: String::new(),
                content: big_content.clone(),
            },
            ExtractedPage {
                url: "https://b.com".into(),
                title: "Page B".into(),
                snippet: String::new(),
                content: big_content,
            },
        ];
        let (ctx, included) = build_context(&pages);
        // First page fits, second should be truncated stub
        assert!(ctx.contains("Page A"));
        assert!(ctx.contains("Page B"));
        assert!(ctx.contains("[Content truncated due to size]"));
        assert!(ctx.len() <= 35_000); // first page ~25K + headers; second is stub
        assert_eq!(included, 2);
    }

    #[test]
    fn test_build_context_empty_pages() {
        let pages: Vec<ExtractedPage> = vec![];
        let (context, count) = build_context(&pages);
        assert!(context.is_empty());
        assert_eq!(count, 0);
    }

    #[test]
    fn test_build_context_included_count_with_truncation() {
        let big_content = "x".repeat(20_000);
        let pages = vec![
            ExtractedPage {
                url: "https://a.com".into(),
                title: "Page 1".into(),
                snippet: String::new(),
                content: big_content.clone(),
            },
            ExtractedPage {
                url: "https://b.com".into(),
                title: "Page 2".into(),
                snippet: String::new(),
                content: big_content.clone(),
            },
            ExtractedPage {
                url: "https://c.com".into(),
                title: "Page 3".into(),
                snippet: String::new(),
                content: "small".into(),
            },
        ];
        let (context, count) = build_context(&pages);
        // First page (20K) fits, second page gets truncated but partially included
        // Third page doesn't fit at all
        assert!(count <= 3);
        assert!(count >= 1);
        assert!(context.len() <= 35_000);
    }

    #[test]
    fn test_build_context_small_pages() {
        let pages = vec![
            ExtractedPage {
                url: "https://a.com".into(),
                title: "A".into(),
                snippet: String::new(),
                content: "hello".into(),
            },
            ExtractedPage {
                url: "https://b.com".into(),
                title: "B".into(),
                snippet: String::new(),
                content: "world".into(),
            },
        ];
        let (ctx, included) = build_context(&pages);
        assert!(ctx.contains("hello"));
        assert!(ctx.contains("world"));
        assert!(!ctx.contains("[Content truncated"));
        assert_eq!(included, 2);
    }
}
