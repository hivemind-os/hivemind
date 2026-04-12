use std::collections::VecDeque;

/// Status returned by [`StallDetector::record`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StallStatus {
    /// The agent is making forward progress.
    Ok,
    /// The agent appears stuck — the same tool call has been repeated
    /// `repeated_count` consecutive times within the sliding window.
    Stalled { tool_name: String, repeated_count: usize },
}

/// Detects when an agent is stuck in a loop by tracking a sliding window
/// of recent `(tool_name, arguments)` pairs and counting **consecutive**
/// identical entries from the tail.
#[derive(Debug, Clone)]
pub struct StallDetector {
    /// Ring buffer of `(tool_name, canonical_args_json)`.
    window: VecDeque<(String, String)>,
    /// Maximum number of entries kept in the window.
    window_size: usize,
    /// Number of consecutive identical entries that triggers a stall.
    threshold: usize,
}

impl StallDetector {
    /// Create a new detector.
    ///
    /// * `window_size` — how many recent calls to track (≥ 1).
    /// * `threshold` — how many consecutive identical calls constitutes a stall (≥ 2).
    pub fn new(window_size: usize, threshold: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size),
            window_size: window_size.max(1),
            threshold: threshold.max(2),
        }
    }

    /// Construct from a [`ToolLimitsConfig`](hive_contracts::ToolLimitsConfig).
    pub fn from_config(config: &hive_contracts::ToolLimitsConfig) -> Self {
        Self::new(config.stall_window, config.stall_threshold)
    }

    /// Record a tool call and check for stalling.
    ///
    /// Returns [`StallStatus::Stalled`] if the exact `(name, args)` pair
    /// appears at least `threshold` **consecutive** times from the tail of
    /// the window (including this new entry).
    pub fn record(&mut self, tool_name: &str, arguments: &serde_json::Value) -> StallStatus {
        let canonical_args = serde_json::to_string(arguments).unwrap_or_default();
        let entry = (tool_name.to_string(), canonical_args);

        // Evict oldest entry if window is full.
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        self.window.push_back(entry.clone());

        // Count consecutive identical entries from the tail.
        let count = self.window.iter().rev().take_while(|e| **e == entry).count();

        if count >= self.threshold {
            StallStatus::Stalled { tool_name: tool_name.to_string(), repeated_count: count }
        } else {
            StallStatus::Ok
        }
    }

    /// Record a batch of tool calls and return the first stall detected (if any).
    pub fn record_batch(&mut self, calls: &[(String, serde_json::Value)]) -> StallStatus {
        for (name, args) in calls {
            let status = self.record(name, args);
            if matches!(status, StallStatus::Stalled { .. }) {
                return status;
            }
        }
        StallStatus::Ok
    }

    /// Clear all recorded history.
    pub fn reset(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diverse_calls_are_ok() {
        let mut d = StallDetector::new(5, 3);
        assert_eq!(d.record("read_file", &json!({"path": "a.rs"})), StallStatus::Ok);
        assert_eq!(d.record("write_file", &json!({"path": "b.rs"})), StallStatus::Ok);
        assert_eq!(d.record("shell", &json!({"cmd": "ls"})), StallStatus::Ok);
    }

    #[test]
    fn consecutive_repeated_calls_trigger_stall() {
        let mut d = StallDetector::new(10, 3);
        let args = json!({"path": "a.rs"});
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(
            d.record("read_file", &args),
            StallStatus::Stalled { tool_name: "read_file".into(), repeated_count: 3 }
        );
    }

    #[test]
    fn non_consecutive_duplicates_are_ok() {
        let mut d = StallDetector::new(10, 3);
        let args = json!({"path": "a.rs"});
        // read, write, read, write, read → 3 total but never 3 consecutive
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(d.record("write_file", &json!({})), StallStatus::Ok);
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(d.record("write_file", &json!({})), StallStatus::Ok);
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
    }

    #[test]
    fn window_eviction_forgets_old_duplicates() {
        let mut d = StallDetector::new(3, 3);
        let args = json!({"path": "a.rs"});
        // Fill window: [read, read, other]
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
        assert_eq!(d.record("write_file", &json!({})), StallStatus::Ok);
        // Window is [read, other, read] — consecutive from tail = 1
        assert_eq!(d.record("read_file", &args), StallStatus::Ok);
    }

    #[test]
    fn same_tool_different_args_is_ok() {
        let mut d = StallDetector::new(10, 3);
        assert_eq!(d.record("read_file", &json!({"path": "a.rs"})), StallStatus::Ok);
        assert_eq!(d.record("read_file", &json!({"path": "b.rs"})), StallStatus::Ok);
        assert_eq!(d.record("read_file", &json!({"path": "c.rs"})), StallStatus::Ok);
    }

    #[test]
    fn threshold_must_be_at_least_two() {
        let mut d = StallDetector::new(10, 1); // clamped to 2
        let args = json!({});
        assert_eq!(d.record("tool", &args), StallStatus::Ok); // count=1 < threshold=2
        assert_eq!(
            d.record("tool", &args),
            StallStatus::Stalled { tool_name: "tool".into(), repeated_count: 2 }
        );
    }

    #[test]
    fn reset_clears_history() {
        let mut d = StallDetector::new(10, 3);
        let args = json!({});
        d.record("tool", &args);
        d.record("tool", &args);
        d.reset();
        // After reset, only 1 in window
        assert_eq!(d.record("tool", &args), StallStatus::Ok);
    }

    #[test]
    fn record_batch_returns_first_stall() {
        let mut d = StallDetector::new(10, 2);
        let calls = vec![
            ("tool".to_string(), json!({"a": 1})),
            ("tool".to_string(), json!({"a": 1})), // triggers stall
            ("other".to_string(), json!({})),
        ];
        assert!(matches!(d.record_batch(&calls), StallStatus::Stalled { .. }));
    }

    #[test]
    fn from_config_uses_config_values() {
        let config = hive_contracts::ToolLimitsConfig {
            stall_window: 7,
            stall_threshold: 4,
            ..Default::default()
        };
        let d = StallDetector::from_config(&config);
        assert_eq!(d.window_size, 7);
        assert_eq!(d.threshold, 4);
    }
}
