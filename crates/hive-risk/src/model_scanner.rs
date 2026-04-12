//! Model-based prompt injection scanner using an isolated LLM.
//!
//! The scanner makes LLM calls via [`ModelRouter`] with no tools and no
//! conversation history, so even if a payload contains injection instructions
//! the model cannot execute them.

use crate::scanner_prompt::{
    format_batch_payload, format_single_payload, parse_batch_verdicts, parse_single_verdict,
};
use arc_swap::ArcSwap;
use hive_contracts::risk::{FlaggedSpan, RiskVerdict};
use hive_core::ScannerModelEntry;
use hive_model::{CompletionMessage, CompletionRequest, ModelRouter};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Result of analysing a single payload.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub verdict: RiskVerdict,
    pub confidence: f32,
    pub threat_type: Option<String>,
    pub flagged_spans: Vec<FlaggedSpan>,
    /// The model that performed the scan (e.g. "azure:gpt-4o-mini").
    pub model_used: String,
    /// How long the scan took in milliseconds.
    pub scan_duration_ms: u64,
}

/// An isolated LLM-based scanner that classifies payloads for prompt injection.
#[derive(Clone)]
pub struct ModelBasedScanner {
    model_router: Arc<ArcSwap<ModelRouter>>,
    preferred_models: Vec<String>,
    max_payload_tokens: usize,
}

impl ModelBasedScanner {
    /// Create a new scanner.
    ///
    /// `scanner_models` is converted to `"{provider}:{model}"` patterns for
    /// the routing request's `preferred_models` field.
    pub fn new(
        model_router: Arc<ArcSwap<ModelRouter>>,
        scanner_models: &[ScannerModelEntry],
        max_payload_tokens: usize,
    ) -> Self {
        let preferred_models =
            scanner_models.iter().map(|m| format!("{}:{}", m.provider, m.model)).collect();
        Self { model_router, preferred_models, max_payload_tokens }
    }

    /// Scan a single payload.
    pub async fn scan_single(&self, content: &str, source: &str) -> Result<AnalysisResult, String> {
        let started = now_ms();

        let system_prompt = crate::scanner_prompt::single_payload_system_prompt().to_string();
        let user_message = format_single_payload(content, source, self.max_payload_tokens);

        let request = CompletionRequest {
            prompt: user_message,
            prompt_content_parts: vec![],
            messages: vec![CompletionMessage {
                role: "system".to_string(),
                content: system_prompt,
                content_parts: vec![],
            }],
            required_capabilities: BTreeSet::new(),
            preferred_models: Some(self.preferred_models.clone()),
            tools: vec![],
        };

        let router = self.model_router.load();
        let response =
            router.complete(&request).map_err(|e| format!("scanner model call failed: {e}"))?;

        let model_used = format!("{}:{}", response.provider_id, response.model);
        let parsed = parse_single_verdict(&response.content)?;
        let duration = now_ms() - started;

        Ok(AnalysisResult {
            verdict: parsed.verdict,
            confidence: parsed.confidence,
            threat_type: parsed.threat_type,
            flagged_spans: parsed.flagged_spans,
            model_used,
            scan_duration_ms: duration,
        })
    }

    /// Scan multiple payloads. Small payloads are batched into a single LLM
    /// call; large payloads are scanned individually in parallel.
    pub async fn scan_batch(
        &self,
        payloads: &[(String, String)], // (content, source)
    ) -> Result<Vec<AnalysisResult>, String> {
        if payloads.is_empty() {
            return Ok(vec![]);
        }
        if payloads.len() == 1 {
            let result = self.scan_single(&payloads[0].0, &payloads[0].1).await?;
            return Ok(vec![result]);
        }

        // Partition into small and large payloads
        let char_limit = self.max_payload_tokens * 4;
        let mut small_indices = Vec::new();
        let mut large_indices = Vec::new();
        for (i, (content, _)) in payloads.iter().enumerate() {
            if content.len() <= char_limit {
                small_indices.push(i);
            } else {
                large_indices.push(i);
            }
        }

        let mut results: Vec<Option<AnalysisResult>> = vec![None; payloads.len()];

        // Batch small payloads into one call
        if !small_indices.is_empty() {
            let batch_payloads: Vec<(String, String)> =
                small_indices.iter().map(|&i| payloads[i].clone()).collect();
            let batch_results = self.scan_batch_inner(&batch_payloads).await?;
            for (batch_idx, &orig_idx) in small_indices.iter().enumerate() {
                if batch_idx < batch_results.len() {
                    results[orig_idx] = Some(batch_results[batch_idx].clone());
                }
            }
        }

        // Scan large payloads individually in parallel
        if !large_indices.is_empty() {
            let futures: Vec<_> = large_indices
                .iter()
                .map(|&i| {
                    let scanner = self.clone();
                    let content = payloads[i].0.clone();
                    let source = payloads[i].1.clone();
                    async move { (i, scanner.scan_single(&content, &source).await) }
                })
                .collect();

            let joined = futures_util::future::join_all(futures).await;
            for (idx, result) in joined {
                results[idx] = Some(result?);
            }
        }

        // Unwrap all results (every index should be filled)
        results
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.ok_or_else(|| format!("missing result for payload {i}")))
            .collect()
    }

    /// Internal: send a batch of small payloads as one LLM call.
    async fn scan_batch_inner(
        &self,
        payloads: &[(String, String)],
    ) -> Result<Vec<AnalysisResult>, String> {
        let started = now_ms();

        let system_prompt = crate::scanner_prompt::batch_payload_system_prompt().to_string();
        let user_message = format_batch_payload(payloads, self.max_payload_tokens);

        let request = CompletionRequest {
            prompt: user_message,
            prompt_content_parts: vec![],
            messages: vec![CompletionMessage {
                role: "system".to_string(),
                content: system_prompt,
                content_parts: vec![],
            }],
            required_capabilities: BTreeSet::new(),
            preferred_models: Some(self.preferred_models.clone()),
            tools: vec![],
        };

        let router = self.model_router.load();
        let response = router
            .complete(&request)
            .map_err(|e| format!("scanner batch model call failed: {e}"))?;

        let model_used = format!("{}:{}", response.provider_id, response.model);
        let duration = now_ms() - started;

        let verdicts = parse_batch_verdicts(&response.content, payloads.len())?;

        Ok(verdicts
            .into_iter()
            .map(|v| AnalysisResult {
                verdict: v.verdict,
                confidence: v.confidence,
                threat_type: v.threat_type,
                flagged_spans: v.flagged_spans,
                model_used: model_used.clone(),
                scan_duration_ms: duration,
            })
            .collect())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_models_from_scanner_entries() {
        let entries = vec![
            ScannerModelEntry { provider: "azure".into(), model: "gpt-4o-mini".into() },
            ScannerModelEntry { provider: "ollama-local".into(), model: "llama3".into() },
        ];
        let scanner = ModelBasedScanner {
            model_router: Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
            preferred_models: entries
                .iter()
                .map(|m| format!("{}:{}", m.provider, m.model))
                .collect(),
            max_payload_tokens: 4096,
        };
        assert_eq!(scanner.preferred_models, vec!["azure:gpt-4o-mini", "ollama-local:llama3"]);
    }
}
