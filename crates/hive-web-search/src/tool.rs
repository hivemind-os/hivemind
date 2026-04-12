use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolApproval, ToolDefinition, ToolDefinitionBuilder, WebSearchConfig};
use hive_model::ModelRouter;
use hive_tools::{BoxFuture, Tool, ToolError, ToolResult};
use serde_json::{json, Value};

use crate::backend::{BraveBackend, SearchBackend, TavilyBackend};
use crate::extract::ContentExtractor;
use crate::synthesize::SearchSynthesizer;

/// Built-in tool that searches the web and returns a synthesized answer.
pub struct WebSearchTool {
    definition: ToolDefinition,
    backend: Arc<dyn SearchBackend>,
    extractor: ContentExtractor,
    synthesizer: SearchSynthesizer,
}

impl WebSearchTool {
    /// Create a WebSearchTool from a [`WebSearchConfig`].
    ///
    /// Returns `None` if the provider is `"none"` or the API key is missing.
    pub fn from_config(
        config: &WebSearchConfig,
        router: Arc<ModelRouter>,
        preferred_models: Option<Vec<String>>,
    ) -> Option<Self> {
        let api_key = config.resolve_api_key()?;
        let backend: Arc<dyn SearchBackend> = match config.provider.as_str() {
            "brave" => Arc::new(
                BraveBackend::new(api_key)
                    .map_err(|e| {
                        tracing::warn!("Failed to initialize Brave search backend: {}", e);
                        e
                    })
                    .ok()?,
            ),
            "tavily" => Arc::new(
                TavilyBackend::new(api_key)
                    .map_err(|e| {
                        tracing::warn!("Failed to initialize Tavily search backend: {}", e);
                        e
                    })
                    .ok()?,
            ),
            "none" | "" => return None,
            other => {
                tracing::warn!(
                    provider = other,
                    "unknown web search provider; web search disabled"
                );
                return None;
            }
        };
        Some(Self::with_backend(router, backend, preferred_models))
    }

    /// Create a WebSearchTool with a custom search backend.
    pub fn with_backend(
        router: Arc<ModelRouter>,
        backend: Arc<dyn SearchBackend>,
        preferred_models: Option<Vec<String>>,
    ) -> Self {
        Self {
            definition: ToolDefinitionBuilder::new("web.search", "Web Search")
                .description(
                    "Search the web for information. Takes a natural language question and \
                     returns a synthesized answer with citations from multiple web sources. \
                     Use this to research libraries, frameworks, best practices, APIs, \
                     documentation, and any other information available on the web.",
                )
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language search question or topic to research."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of web results to fetch and synthesize (default: 5, max: 10).",
                            "default": 5,
                            "minimum": 1,
                            "maximum": 10
                        }
                    },
                    "required": ["query"]
                }))
                .output_schema(json!({
                    "type": "object",
                    "properties": {
                        "answer": {
                            "type": "string",
                            "description": "Synthesized answer with inline [N] citations."
                        },
                        "sources": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "index": { "type": "integer" },
                                    "title": { "type": "string" },
                                    "url": { "type": "string" },
                                    "snippet": { "type": "string" }
                                }
                            }
                        }
                    }
                }))
                .channel_class(ChannelClass::Public)
                .read_only()
                .approval(ToolApproval::Auto)
                .build(),
            backend,
            extractor: ContentExtractor::default(),
            synthesizer: SearchSynthesizer::new(router, preferred_models),
        }
    }
}

impl Tool for WebSearchTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing required field `query`".into()))?;

            let max_results = input
                .get("max_results")
                .and_then(|v| v.as_u64())
                .map(|n| n.clamp(1, 10) as usize)
                .unwrap_or(5);

            tracing::info!(query = %query, max_results, "performing web search");

            // Step 1: Search
            let search_results = self
                .backend
                .search(query, max_results)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("search failed: {e}")))?;

            if search_results.is_empty() {
                return Ok(ToolResult {
                    output: json!({
                        "answer": "No search results found for the given query.",
                        "sources": []
                    }),
                    data_class: DataClass::Public,
                });
            }

            // Step 2: Extract content from pages
            let pages = self.extractor.extract_content(&search_results).await;

            // Step 3: Synthesize with model (blocking call — run off async worker)
            let synthesizer = self.synthesizer.clone();
            let query_owned = query.to_string();
            let synthesis =
                tokio::task::spawn_blocking(move || synthesizer.synthesize(&query_owned, &pages))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("synthesis task failed: {e}")))?
                    .map_err(|e| ToolError::ExecutionFailed(format!("synthesis failed: {e}")))?;

            Ok(ToolResult {
                output: json!({
                    "answer": synthesis.answer,
                    "sources": synthesis.sources,
                }),
                data_class: DataClass::Public,
            })
        })
    }
}
