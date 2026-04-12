use std::collections::HashSet;

use crate::error::CanvasError;
use crate::store::CanvasStore;
use crate::token_counter::TokenCounter;
use crate::types::{CanvasNode, CardType};

/// A card selected for inclusion in the LLM context, with priority metadata.
#[derive(Clone, Debug)]
pub struct ContextCard {
    pub node: CanvasNode,
    pub priority: ContextPriority,
    pub token_estimate: usize,
}

/// Priority levels for context cards, ordered from most to least important.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextPriority {
    /// Must include — directly connected via edge.
    Required,
    /// High priority — spatially close.
    High,
    /// Medium — in the same cluster.
    Medium,
    /// Low — reachable via graph traversal.
    Low,
}

/// Assembles context cards for LLM prompts using spatial proximity and graph
/// traversal on the canvas. Cards are ranked by priority and trimmed to fit a
/// token budget.
pub struct SpatialContextAssembler<'a, S: CanvasStore + ?Sized, T: TokenCounter> {
    store: &'a S,
    token_counter: T,
    proximity_radius: f64,
    max_graph_depth: usize,
}

impl<'a, S: CanvasStore + ?Sized, T: TokenCounter> SpatialContextAssembler<'a, S, T> {
    pub fn new(store: &'a S, token_counter: T) -> Self {
        Self { store, token_counter, proximity_radius: 500.0, max_graph_depth: 3 }
    }

    pub fn with_radius(mut self, radius: f64) -> Self {
        self.proximity_radius = radius;
        self
    }

    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_graph_depth = depth;
        self
    }

    /// Assemble context cards for a prompt at the given node, within token budget.
    ///
    /// Layers (in priority order):
    /// 1. **Required** – nodes directly connected via edges
    /// 2. **High**     – nodes within `proximity_radius` of the prompt node
    /// 3. **Medium**   – members of nearby Cluster nodes (2× proximity radius)
    /// 4. **Low**      – nodes reachable via BFS up to `max_graph_depth`
    ///
    /// Required-priority cards are always included even if they exceed the budget.
    pub fn assemble(
        &self,
        prompt_node: &CanvasNode,
        token_budget: usize,
    ) -> Result<Vec<ContextCard>, CanvasError> {
        let canvas_id = &prompt_node.canvas_id;
        let mut seen = HashSet::new();
        seen.insert(prompt_node.id.clone());

        let mut candidates: Vec<ContextCard> = Vec::new();

        // Layer 1: Direct edges → Required priority
        let edges_out = self.store.get_edges_from(&prompt_node.id)?;
        let edges_in = self.store.get_edges_to(&prompt_node.id)?;
        for edge in edges_out.iter().chain(edges_in.iter()) {
            let target_id =
                if edge.source_id == prompt_node.id { &edge.target_id } else { &edge.source_id };
            if seen.insert(target_id.clone()) {
                if let Some(node) = self.store.get_node(target_id)? {
                    let tokens = self.token_counter.count(&node.content.to_string());
                    candidates.push(ContextCard {
                        node,
                        priority: ContextPriority::Required,
                        token_estimate: tokens,
                    });
                }
            }
        }

        // Layer 2: Spatial proximity → High priority
        let nearby = self.store.query_radius(
            canvas_id,
            prompt_node.x,
            prompt_node.y,
            self.proximity_radius,
        )?;
        for node in nearby {
            if seen.insert(node.id.clone()) {
                let tokens = self.token_counter.count(&node.content.to_string());
                candidates.push(ContextCard {
                    node,
                    priority: ContextPriority::High,
                    token_estimate: tokens,
                });
            }
        }

        // Layer 3: Cluster membership → Medium priority
        // Find Cluster nodes nearby, then include their connected members.
        let all_nearby = self.store.query_radius(
            canvas_id,
            prompt_node.x,
            prompt_node.y,
            self.proximity_radius * 2.0,
        )?;
        let cluster_nodes: Vec<&CanvasNode> =
            all_nearby.iter().filter(|n| n.card_type == CardType::Cluster).collect();
        for cluster in cluster_nodes {
            let cluster_edges = self.store.get_edges_from(&cluster.id)?;
            for edge in &cluster_edges {
                if seen.insert(edge.target_id.clone()) {
                    if let Some(node) = self.store.get_node(&edge.target_id)? {
                        let tokens = self.token_counter.count(&node.content.to_string());
                        candidates.push(ContextCard {
                            node,
                            priority: ContextPriority::Medium,
                            token_estimate: tokens,
                        });
                    }
                }
            }
            // Include the cluster node itself
            if seen.insert(cluster.id.clone()) {
                let tokens = self.token_counter.count(&cluster.content.to_string());
                candidates.push(ContextCard {
                    node: cluster.clone(),
                    priority: ContextPriority::Medium,
                    token_estimate: tokens,
                });
            }
        }

        // Layer 4: Extended graph BFS → Low priority
        let graph_nodes = self.store.bfs(&prompt_node.id, self.max_graph_depth)?;
        for node in graph_nodes {
            if seen.insert(node.id.clone()) {
                let tokens = self.token_counter.count(&node.content.to_string());
                candidates.push(ContextCard {
                    node,
                    priority: ContextPriority::Low,
                    token_estimate: tokens,
                });
            }
        }

        // Sort by priority (Required first → High → Medium → Low)
        candidates.sort_by(|a, b| a.priority.cmp(&b.priority));

        // Trim to token budget, always keeping Required cards
        let mut total_tokens = 0;
        let mut result = Vec::new();
        for card in candidates {
            if card.priority == ContextPriority::Required
                || total_tokens + card.token_estimate <= token_budget
            {
                total_tokens += card.token_estimate;
                result.push(card);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanvasEdge, CanvasNode, CardStatus, CardType, EdgeType, SqliteCanvasStore};

    fn make_node(id: &str, canvas_id: &str, x: f64, y: f64, content: &str) -> CanvasNode {
        CanvasNode {
            id: id.into(),
            canvas_id: canvas_id.into(),
            card_type: CardType::Response,
            x,
            y,
            width: 280.0,
            height: 120.0,
            content: serde_json::json!({ "text": content }),
            status: CardStatus::Active,
            created_by: "test".into(),
            created_at: 0,
        }
    }

    fn make_edge(id: &str, canvas_id: &str, source: &str, target: &str) -> CanvasEdge {
        CanvasEdge {
            id: id.into(),
            canvas_id: canvas_id.into(),
            source_id: source.into(),
            target_id: target.into(),
            edge_type: EdgeType::ReplyTo,
            metadata: serde_json::json!({}),
            created_at: 0,
        }
    }

    use crate::token_counter::ApproxTokenCounter;

    fn setup() -> (SqliteCanvasStore, ApproxTokenCounter) {
        let store = SqliteCanvasStore::in_memory().unwrap();
        (store, ApproxTokenCounter)
    }

    #[test]
    fn test_empty_canvas_returns_empty() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 0.0, 0.0, "hello");
        store.insert_node(&prompt).unwrap();
        let asm = SpatialContextAssembler::new(&store, tc);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert!(cards.is_empty(), "No neighbors → empty result");
    }

    #[test]
    fn test_direct_edges_are_required_priority() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 0.0, 0.0, "question");
        let reply = make_node("r1", "c1", 5000.0, 5000.0, "answer");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&reply).unwrap();
        store.insert_edge(&make_edge("e1", "c1", "p1", "r1")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(100.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].node.id, "r1");
        assert_eq!(cards[0].priority, ContextPriority::Required);
    }

    #[test]
    fn test_incoming_edges_are_required_priority() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 0.0, 0.0, "question");
        let source = make_node("s1", "c1", 5000.0, 5000.0, "source");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&source).unwrap();
        // Edge goes *to* prompt, not from it
        store.insert_edge(&make_edge("e1", "c1", "s1", "p1")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(100.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].node.id, "s1");
        assert_eq!(cards[0].priority, ContextPriority::Required);
    }

    #[test]
    fn test_spatial_proximity_is_high_priority() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 100.0, 100.0, "prompt");
        let nearby = make_node("n1", "c1", 120.0, 120.0, "nearby");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&nearby).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].node.id, "n1");
        assert_eq!(cards[0].priority, ContextPriority::High);
    }

    #[test]
    fn test_distant_nodes_excluded() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 0.0, 0.0, "prompt");
        let far = make_node("f1", "c1", 9999.0, 9999.0, "far away");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&far).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert!(cards.is_empty(), "Far-away node should not appear");
    }

    #[test]
    fn test_graph_bfs_is_low_priority() {
        let (store, tc) = setup();
        // Chain: p1 → a → b → c, all spatially distant
        let p1 = make_node("p1", "c1", 0.0, 0.0, "prompt");
        let a = make_node("a", "c1", 2000.0, 2000.0, "node A");
        let b = make_node("b", "c1", 4000.0, 4000.0, "node B");
        let c = make_node("c", "c1", 6000.0, 6000.0, "node C");
        for n in [&p1, &a, &b, &c] {
            store.insert_node(n).unwrap();
        }
        store.insert_edge(&make_edge("e1", "c1", "p1", "a")).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "a", "b")).unwrap();
        store.insert_edge(&make_edge("e3", "c1", "b", "c")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc)
            .with_radius(100.0) // small radius — no spatial hits
            .with_max_depth(3);
        let cards = asm.assemble(&p1, 10000).unwrap();

        // "a" is Required (direct edge), "b" and "c" are Low (BFS)
        let required: Vec<_> =
            cards.iter().filter(|c| c.priority == ContextPriority::Required).collect();
        let low: Vec<_> = cards.iter().filter(|c| c.priority == ContextPriority::Low).collect();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].node.id, "a");
        assert_eq!(low.len(), 2);
        let low_ids: HashSet<_> = low.iter().map(|c| c.node.id.as_str()).collect();
        assert!(low_ids.contains("b"));
        assert!(low_ids.contains("c"));
    }

    #[test]
    fn test_token_budget_respected() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 100.0, 100.0, "prompt");
        store.insert_node(&prompt).unwrap();

        // Create 10 nearby nodes, each with ~25 token content (100 chars)
        let content = "x".repeat(100); // 100 chars → 25 tokens
        for i in 0..10 {
            let n = make_node(&format!("n{i}"), "c1", 110.0 + i as f64, 110.0 + i as f64, &content);
            store.insert_node(&n).unwrap();
        }

        // Budget = 100 tokens → should fit roughly 3-4 nodes (each ~25+ tokens from JSON wrapping)
        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 100).unwrap();
        assert!(cards.len() < 10, "Should not include all 10 nodes");

        let total: usize = cards.iter().map(|c| c.token_estimate).sum();
        assert!(total <= 100, "Total tokens {total} should be ≤ budget 100");
    }

    #[test]
    fn test_required_always_included_over_budget() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 0.0, 0.0, "prompt");
        let reply = make_node("r1", "c1", 5000.0, 5000.0, &"x".repeat(200));
        store.insert_node(&prompt).unwrap();
        store.insert_node(&reply).unwrap();
        store.insert_edge(&make_edge("e1", "c1", "p1", "r1")).unwrap();

        // Budget of 1 token — Required card should still be included
        let asm = SpatialContextAssembler::new(&store, tc).with_radius(100.0);
        let cards = asm.assemble(&prompt, 1).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].priority, ContextPriority::Required);
    }

    #[test]
    fn test_deduplication() {
        let (store, tc) = setup();
        // Node that is both edge-connected AND spatially close
        let prompt = make_node("p1", "c1", 100.0, 100.0, "prompt");
        let both = make_node("b1", "c1", 110.0, 110.0, "both");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&both).unwrap();
        store.insert_edge(&make_edge("e1", "c1", "p1", "b1")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert_eq!(cards.len(), 1, "Should appear only once");
        assert_eq!(cards[0].priority, ContextPriority::Required);
    }

    #[test]
    fn test_priority_ordering() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 100.0, 100.0, "prompt");
        // Required: edge-connected, far away
        let req = make_node("req", "c1", 5000.0, 5000.0, "required");
        // High: spatially close, no edge
        let high = make_node("high", "c1", 120.0, 120.0, "high");
        // Low: reachable via BFS through req, far away
        let low = make_node("low", "c1", 8000.0, 8000.0, "low");

        for n in [&prompt, &req, &high, &low] {
            store.insert_node(n).unwrap();
        }
        store.insert_edge(&make_edge("e1", "c1", "p1", "req")).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "req", "low")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();

        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].priority, ContextPriority::Required);
        assert_eq!(cards[1].priority, ContextPriority::High);
        assert_eq!(cards[2].priority, ContextPriority::Low);
    }

    #[test]
    fn test_prompt_node_excluded_from_results() {
        let (store, tc) = setup();
        let prompt = make_node("p1", "c1", 100.0, 100.0, "prompt");
        let nearby = make_node("n1", "c1", 110.0, 110.0, "nearby");
        store.insert_node(&prompt).unwrap();
        store.insert_node(&nearby).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0);
        let cards = asm.assemble(&prompt, 10000).unwrap();
        assert!(
            !cards.iter().any(|c| c.node.id == "p1"),
            "Prompt node itself should not be in results"
        );
    }

    #[test]
    fn test_full_scenario_auth_cluster() {
        let (store, tc) = setup();

        // Auth cluster: 10 nodes at (100, 100) region
        for i in 0..10 {
            let n = make_node(
                &format!("auth_{i}"),
                "c1",
                100.0 + (i as f64 * 20.0),
                100.0 + (i as f64 * 10.0),
                &format!("auth operation {i}"),
            );
            store.insert_node(&n).unwrap();
        }
        // Chain edges within auth cluster: auth_0 → auth_1 → ... → auth_9
        for i in 0..9 {
            store
                .insert_edge(&make_edge(
                    &format!("ae_{i}"),
                    "c1",
                    &format!("auth_{i}"),
                    &format!("auth_{}", i + 1),
                ))
                .unwrap();
        }

        // Pipeline cluster: 10 nodes at (5000, 5000) region — far away
        for i in 0..10 {
            let n = make_node(
                &format!("pipe_{i}"),
                "c1",
                5000.0 + (i as f64 * 20.0),
                5000.0 + (i as f64 * 10.0),
                &format!("pipeline step {i}"),
            );
            store.insert_node(&n).unwrap();
        }
        for i in 0..9 {
            store
                .insert_edge(&make_edge(
                    &format!("pe_{i}"),
                    "c1",
                    &format!("pipe_{i}"),
                    &format!("pipe_{}", i + 1),
                ))
                .unwrap();
        }

        // Prompt at auth region
        let prompt = make_node("prompt", "c1", 150.0, 130.0, "user question about auth");
        store.insert_node(&prompt).unwrap();
        // Edge from prompt to auth_0
        store.insert_edge(&make_edge("ep", "c1", "prompt", "auth_0")).unwrap();

        let asm = SpatialContextAssembler::new(&store, tc).with_radius(500.0).with_max_depth(3);
        let cards = asm.assemble(&prompt, 10000).unwrap();

        let auth_count = cards.iter().filter(|c| c.node.id.starts_with("auth")).count();
        let pipe_count = cards.iter().filter(|c| c.node.id.starts_with("pipe")).count();

        assert!(auth_count >= 5, "Should include most auth nodes, got {auth_count}");
        assert_eq!(pipe_count, 0, "Pipeline nodes should be excluded");
    }
}
