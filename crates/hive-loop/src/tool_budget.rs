use hive_contracts::ToolLimitsConfig;

/// Decision returned by [`AdaptiveBudget::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDecision {
    /// The proposed tool calls are within the current budget — proceed.
    Allow,
    /// The soft limit was reached but the agent is making progress.
    /// The budget has been extended.
    Extended { new_budget: usize, extensions_granted: usize },
    /// The hard ceiling would be exceeded — stop the agent.
    HardStop { ceiling: usize },
}

/// Adaptive tool-call budget that auto-extends when the agent is making
/// forward progress, up to a hard ceiling.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AdaptiveBudget {
    soft_limit: usize,
    hard_ceiling: usize,
    extension_chunk: usize,
    current_budget: usize,
    extensions_granted: usize,
}

impl AdaptiveBudget {
    /// Create from config.
    pub fn new(config: &ToolLimitsConfig) -> Self {
        Self {
            soft_limit: config.soft_limit,
            hard_ceiling: config.hard_ceiling,
            extension_chunk: config.extension_chunk,
            current_budget: config.soft_limit,
            extensions_granted: 0,
        }
    }

    /// Check whether executing `batch_size` more tool calls is allowed,
    /// given the current cumulative `count`.
    ///
    /// This should be called **before** executing the tool calls.
    pub fn check(&mut self, current_count: usize, batch_size: usize) -> BudgetDecision {
        let proposed = current_count + batch_size;

        // Within current budget — proceed.
        if proposed <= self.current_budget {
            return BudgetDecision::Allow;
        }

        // Over current budget — try to extend.
        // Calculate how much we need and extend in chunks.
        let mut new_budget = self.current_budget;
        while new_budget < proposed && new_budget < self.hard_ceiling {
            new_budget = (new_budget + self.extension_chunk).min(self.hard_ceiling);
            self.extensions_granted += 1;
        }

        if proposed <= new_budget {
            self.current_budget = new_budget;
            BudgetDecision::Extended {
                new_budget: self.current_budget,
                extensions_granted: self.extensions_granted,
            }
        } else {
            // Even after extending to hard ceiling, still not enough.
            self.current_budget = new_budget;
            BudgetDecision::HardStop { ceiling: self.hard_ceiling }
        }
    }

    /// The current effective budget (soft limit + any extensions).
    pub fn current_budget(&self) -> usize {
        self.current_budget
    }

    /// Number of times the budget has been extended.
    pub fn extensions_granted(&self) -> usize {
        self.extensions_granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ToolLimitsConfig {
        ToolLimitsConfig {
            soft_limit: 10,
            hard_ceiling: 30,
            extension_chunk: 10,
            stall_window: 5,
            stall_threshold: 3,
        }
    }

    #[test]
    fn under_soft_limit_allows() {
        let mut b = AdaptiveBudget::new(&default_config());
        assert_eq!(b.check(5, 3), BudgetDecision::Allow);
    }

    #[test]
    fn at_exact_soft_limit_allows() {
        let mut b = AdaptiveBudget::new(&default_config());
        // 10 calls, budget is 10 → exactly at limit
        assert_eq!(b.check(8, 2), BudgetDecision::Allow);
    }

    #[test]
    fn over_soft_limit_extends() {
        let mut b = AdaptiveBudget::new(&default_config());
        let result = b.check(10, 1);
        assert_eq!(result, BudgetDecision::Extended { new_budget: 20, extensions_granted: 1 });
        assert_eq!(b.current_budget(), 20);
    }

    #[test]
    fn multiple_extensions_accumulate() {
        let mut b = AdaptiveBudget::new(&default_config());
        // First extension: 10 → 20
        let r1 = b.check(10, 1);
        assert_eq!(r1, BudgetDecision::Extended { new_budget: 20, extensions_granted: 1 });

        // Within extended budget
        assert_eq!(b.check(15, 3), BudgetDecision::Allow);

        // Second extension: 20 → 30
        let r2 = b.check(20, 1);
        assert_eq!(r2, BudgetDecision::Extended { new_budget: 30, extensions_granted: 2 });
    }

    #[test]
    fn hard_ceiling_stops() {
        let mut b = AdaptiveBudget::new(&default_config());
        // Try to go past hard ceiling (30)
        let result = b.check(30, 1);
        assert_eq!(result, BudgetDecision::HardStop { ceiling: 30 });
    }

    #[test]
    fn large_batch_extends_multiple_chunks() {
        let mut b = AdaptiveBudget::new(&default_config());
        // Batch of 15 from count=5 → proposed=20, needs 2 chunks (10→20)
        let result = b.check(5, 15);
        assert_eq!(result, BudgetDecision::Extended { new_budget: 20, extensions_granted: 1 });
    }

    #[test]
    fn large_batch_exceeds_ceiling() {
        let config = ToolLimitsConfig {
            soft_limit: 10,
            hard_ceiling: 15,
            extension_chunk: 10,
            ..default_config()
        };
        let mut b = AdaptiveBudget::new(&config);
        // Batch of 20 from count=0 → proposed=20 > ceiling=15
        let result = b.check(0, 20);
        assert_eq!(result, BudgetDecision::HardStop { ceiling: 15 });
    }
}
