use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u32,
    pub tool_calls: u32,
    #[serde(default)]
    pub per_model: HashMap<String, ModelUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub per_agent: Vec<(String, TokenUsage)>,
    pub total: TokenUsage,
}

/// Tracks per-agent and aggregate token usage.
pub struct TokenAccumulator {
    per_agent: DashMap<String, TokenUsage>,
    total_input: AtomicU64,
    total_output: AtomicU64,
    total_model_calls: AtomicU64,
    total_tool_calls: AtomicU64,
}

impl TokenAccumulator {
    pub fn new(_budget_limit_usd: Option<f64>) -> Self {
        Self {
            per_agent: DashMap::new(),
            total_input: AtomicU64::new(0),
            total_output: AtomicU64::new(0),
            total_model_calls: AtomicU64::new(0),
            total_tool_calls: AtomicU64::new(0),
        }
    }

    /// Record a model call's token usage with model name.
    pub fn record_model_call(&self, agent_id: &str, model: &str, output_tokens: u64) {
        let mut entry = self.per_agent.entry(agent_id.to_string()).or_default();
        entry.output_tokens += output_tokens;
        entry.model_calls += 1;

        let model_entry = entry.per_model.entry(model.to_string()).or_default();
        model_entry.output_tokens += output_tokens;
        model_entry.calls += 1;

        self.total_output.fetch_add(output_tokens, Ordering::Relaxed);
        self.total_model_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Record estimated input tokens for a model call.
    pub fn record_input_tokens(&self, agent_id: &str, model: &str, input_tokens: u64) {
        let mut entry = self.per_agent.entry(agent_id.to_string()).or_default();
        entry.input_tokens += input_tokens;

        let model_entry = entry.per_model.entry(model.to_string()).or_default();
        model_entry.input_tokens += input_tokens;

        self.total_input.fetch_add(input_tokens, Ordering::Relaxed);
    }

    /// Record a tool call (no token cost, just count).
    pub fn record_tool_call(&self, agent_id: &str) {
        let mut entry = self.per_agent.entry(agent_id.to_string()).or_default();
        entry.tool_calls += 1;
        self.total_tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a full telemetry snapshot.
    pub fn snapshot(&self) -> TelemetrySnapshot {
        let per_agent: Vec<_> = self
            .per_agent
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        // Aggregate per-model totals
        let mut total_per_model: HashMap<String, ModelUsage> = HashMap::new();
        for pair in self.per_agent.iter() {
            for (model, usage) in &pair.value().per_model {
                let entry = total_per_model.entry(model.clone()).or_default();
                entry.input_tokens += usage.input_tokens;
                entry.output_tokens += usage.output_tokens;
                entry.calls += usage.calls;
            }
        }

        TelemetrySnapshot {
            per_agent,
            total: TokenUsage {
                input_tokens: self.total_input.load(Ordering::Relaxed),
                output_tokens: self.total_output.load(Ordering::Relaxed),
                model_calls: self.total_model_calls.load(Ordering::Relaxed) as u32,
                tool_calls: self.total_tool_calls.load(Ordering::Relaxed) as u32,
                per_model: total_per_model,
            },
        }
    }

    /// Reset all counters.
    pub fn reset(&self) {
        self.per_agent.clear();
        self.total_input.store(0, Ordering::Relaxed);
        self.total_output.store(0, Ordering::Relaxed);
        self.total_model_calls.store(0, Ordering::Relaxed);
        self.total_tool_calls.store(0, Ordering::Relaxed);
    }

    /// Remove a specific agent's per-agent entry.
    /// Totals are left intact since they represent cumulative usage.
    pub fn remove_agent(&self, agent_id: &str) {
        self.per_agent.remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_model_call_updates_totals() {
        let acc = TokenAccumulator::new(None);
        acc.record_model_call("agent-1", "gpt-4o", 50);
        let snap = acc.snapshot();
        assert_eq!(snap.total.output_tokens, 50);
        assert_eq!(snap.total.model_calls, 1);
    }

    #[test]
    fn test_per_agent_tracking() {
        let acc = TokenAccumulator::new(None);
        acc.record_model_call("agent-1", "gpt-4o", 50);
        acc.record_model_call("agent-2", "claude-sonnet", 100);
        acc.record_model_call("agent-1", "gpt-4o", 25);
        let snap = acc.snapshot();
        assert_eq!(snap.per_agent.len(), 2);

        let a1 = snap.per_agent.iter().find(|(id, _)| id == "agent-1").unwrap();
        assert_eq!(a1.1.output_tokens, 75);
        assert_eq!(a1.1.model_calls, 2);

        let a2 = snap.per_agent.iter().find(|(id, _)| id == "agent-2").unwrap();
        assert_eq!(a2.1.output_tokens, 100);
        assert_eq!(a2.1.model_calls, 1);
    }

    #[test]
    fn test_per_model_tracking() {
        let acc = TokenAccumulator::new(None);
        acc.record_model_call("agent-1", "gpt-4o", 50);
        acc.record_model_call("agent-1", "claude-sonnet", 100);
        acc.record_model_call("agent-1", "gpt-4o", 30);
        let snap = acc.snapshot();

        let a1 = snap.per_agent.iter().find(|(id, _)| id == "agent-1").unwrap();
        assert_eq!(a1.1.per_model.len(), 2);
        assert_eq!(a1.1.per_model["gpt-4o"].output_tokens, 80);
        assert_eq!(a1.1.per_model["gpt-4o"].calls, 2);
        assert_eq!(a1.1.per_model["claude-sonnet"].output_tokens, 100);
        assert_eq!(a1.1.per_model["claude-sonnet"].calls, 1);

        // Total should also have per-model
        assert_eq!(snap.total.per_model["gpt-4o"].calls, 2);
        assert_eq!(snap.total.per_model["claude-sonnet"].calls, 1);
    }

    #[test]
    fn test_tool_call_tracking() {
        let acc = TokenAccumulator::new(None);
        acc.record_tool_call("agent-1");
        acc.record_tool_call("agent-1");
        let snap = acc.snapshot();
        assert_eq!(snap.total.tool_calls, 2);
    }

    #[test]
    fn test_reset_clears_all() {
        let acc = TokenAccumulator::new(None);
        acc.record_model_call("a", "gpt-4o", 50);
        acc.record_tool_call("a");
        acc.reset();
        let snap = acc.snapshot();
        assert_eq!(snap.total.input_tokens, 0);
        assert_eq!(snap.total.output_tokens, 0);
        assert_eq!(snap.total.model_calls, 0);
        assert_eq!(snap.total.tool_calls, 0);
        assert_eq!(snap.per_agent.len(), 0);
    }

    #[test]
    fn test_multiple_agents_tool_calls() {
        let acc = TokenAccumulator::new(None);
        acc.record_tool_call("agent-1");
        acc.record_tool_call("agent-2");
        acc.record_tool_call("agent-1");
        let snap = acc.snapshot();
        assert_eq!(snap.total.tool_calls, 3);

        let a1 = snap.per_agent.iter().find(|(id, _)| id == "agent-1").unwrap();
        assert_eq!(a1.1.tool_calls, 2);

        let a2 = snap.per_agent.iter().find(|(id, _)| id == "agent-2").unwrap();
        assert_eq!(a2.1.tool_calls, 1);
    }
}
