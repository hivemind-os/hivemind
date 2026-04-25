use std::collections::HashMap;

use hive_contracts::ReasoningEvent;
use serde_json::json;

use crate::{CanvasEdge, CanvasEvent, CanvasNode, CardStatus, CardType, EdgeType, NodePatch};

/// Auto-layout engine that positions cards in a tree structure.
///
/// Children are placed below their parent with vertical spacing (`Y_SPACING`),
/// and siblings are spread horizontally with horizontal spacing (`X_SPACING`).
struct TreeLayout {
    depth_offsets: HashMap<usize, f64>,
    node_depths: HashMap<String, usize>,
}

const X_SPACING: f64 = 300.0;
const Y_SPACING: f64 = 180.0;

impl TreeLayout {
    fn new() -> Self {
        Self { depth_offsets: HashMap::new(), node_depths: HashMap::new() }
    }

    /// Register the root node at depth 0, position (0, 0).
    fn register_root(&mut self, node_id: &str) {
        self.node_depths.insert(node_id.to_string(), 0);
    }

    /// Calculate position for a new child node of the given parent.
    fn position_child(&mut self, parent_id: &str, child_id: &str) -> (f64, f64) {
        let parent_depth = self.node_depths.get(parent_id).copied().unwrap_or(0);
        let child_depth = parent_depth + 1;

        let x_offset = self.depth_offsets.entry(child_depth).or_insert(0.0);
        let pos = (*x_offset, child_depth as f64 * Y_SPACING);
        *x_offset += X_SPACING;

        self.node_depths.insert(child_id.to_string(), child_depth);
        pos
    }

    /// Position a node with no parent context at depth 0.
    fn position_orphan(&mut self, node_id: &str) -> (f64, f64) {
        let x_offset = self.depth_offsets.entry(0).or_insert(0.0);
        let pos = (*x_offset, 0.0);
        *x_offset += X_SPACING;
        self.node_depths.insert(node_id.to_string(), 0);
        pos
    }
}

struct PendingModel {
    card_id: String,
}

struct PendingTool {
    card_id: String,
}

/// A pure event transformer that converts semantic `ReasoningEvent`s into
/// spatial `CanvasEvent`s. It does **not** hold a `CanvasStore` reference;
/// the caller is responsible for persisting the emitted events.
pub struct DagObserver {
    canvas_id: String,
    card_stack: Vec<String>,
    pending_model: Option<PendingModel>,
    pending_tools: HashMap<String, PendingTool>,
    layout: TreeLayout,
    next_id: u64,
}

impl DagObserver {
    /// Create a new DagObserver and emit the root Prompt card.
    pub fn new(canvas_id: String, root_prompt: &str) -> (Self, Vec<CanvasEvent>) {
        let mut layout = TreeLayout::new();
        let root_id = "card-1".to_string();
        layout.register_root(&root_id);

        let root_node = CanvasNode {
            id: root_id.clone(),
            canvas_id: canvas_id.clone(),
            card_type: CardType::Prompt,
            x: 0.0,
            y: 0.0,
            width: 280.0,
            height: 120.0,
            content: json!({ "text": root_prompt }),
            status: CardStatus::Active,
            created_by: "user".to_string(),
            created_at: 0,
        };

        let events = vec![CanvasEvent::NodeCreated { node: root_node, parent_edge: None }];

        let observer = Self {
            canvas_id,
            card_stack: vec![root_id],
            pending_model: None,
            pending_tools: HashMap::new(),
            layout,
            next_id: 1,
        };

        (observer, events)
    }

    /// Transform a `ReasoningEvent` into zero or more `CanvasEvent`s.
    pub fn observe(&mut self, event: &ReasoningEvent) -> Vec<CanvasEvent> {
        match event {
            ReasoningEvent::StepStarted { step_id, description } => {
                self.handle_step_started(step_id, description)
            }
            ReasoningEvent::ModelCallStarted { model, prompt_preview, .. } => {
                self.handle_model_call_started(model, prompt_preview)
            }
            ReasoningEvent::ModelCallCompleted { content, token_count, .. } => {
                self.handle_model_call_completed(content, *token_count)
            }
            ReasoningEvent::ToolCallStarted { tool_id, input } => {
                self.handle_tool_call_started(tool_id, input)
            }
            ReasoningEvent::ToolCallCompleted { tool_id, output, is_error } => {
                self.handle_tool_call_completed(tool_id, output, *is_error)
            }
            ReasoningEvent::BranchEvaluated { condition, result } => {
                self.handle_branch_evaluated(condition, *result)
            }
            ReasoningEvent::PathAbandoned { reason } => self.handle_path_abandoned(reason),
            ReasoningEvent::Synthesized { sources, result } => {
                self.handle_synthesized(sources, result)
            }
            ReasoningEvent::Completed { result } => self.handle_completed(result),
            ReasoningEvent::Failed { error, .. } => self.handle_failed(error),
            ReasoningEvent::TokenDelta { token } => self.handle_token_delta(token),
            ReasoningEvent::UserInteractionRequired { .. } => vec![],
            ReasoningEvent::QuestionAsked { .. } => vec![],
            ReasoningEvent::ModelRetry { .. } => vec![],
            ReasoningEvent::ToolCallArgDelta { .. } => vec![],
            ReasoningEvent::ToolCallIntercepted { tool_id, input } => {
                // Treat intercepted tool calls like completed tool calls in the DAG
                let mut events = self.handle_tool_call_started(tool_id, input);
                events.extend(self.handle_tool_call_completed(
                    tool_id,
                    &serde_json::json!({"intercepted": true}),
                    false,
                ));
                events
            }
            ReasoningEvent::CodeExecution { code, output, is_error } => {
                // Treat code execution as a tool call in the DAG
                let input = serde_json::json!({"code": code});
                let output_val = serde_json::json!({"output": output});
                let mut events =
                    self.handle_tool_call_started(&"code_execution".to_string(), &input);
                events.extend(self.handle_tool_call_completed(
                    &"code_execution".to_string(),
                    &output_val,
                    *is_error,
                ));
                events
            }
        }
    }

    fn generate_id(&mut self) -> String {
        self.next_id += 1;
        format!("card-{}", self.next_id)
    }

    fn generate_edge_id(&mut self) -> String {
        self.next_id += 1;
        format!("edge-{}", self.next_id)
    }

    fn current_parent(&self) -> Option<&str> {
        self.card_stack.last().map(|s| s.as_str())
    }

    /// Create a CanvasNode positioned as a child of the current parent.
    fn create_child_node(
        &mut self,
        card_type: CardType,
        content: serde_json::Value,
        status: CardStatus,
    ) -> (CanvasNode, Option<CanvasEdge>, String) {
        let card_id = self.generate_id();
        let parent_id = self.current_parent().map(|s| s.to_string());

        let (x, y) = if let Some(ref pid) = parent_id {
            self.layout.position_child(pid, &card_id)
        } else {
            self.layout.position_orphan(&card_id)
        };

        let edge_type = match card_type {
            CardType::ToolCall => EdgeType::ToolIO,
            CardType::Decomposition => EdgeType::DecomposesTo,
            CardType::Synthesis => EdgeType::Synthesizes,
            _ => EdgeType::ReplyTo,
        };

        let node = CanvasNode {
            id: card_id.clone(),
            canvas_id: self.canvas_id.clone(),
            card_type,
            x,
            y,
            width: 280.0,
            height: 120.0,
            content,
            status,
            created_by: "agent".to_string(),
            created_at: 0,
        };

        let edge = parent_id.map(|pid| {
            let edge_id = self.generate_edge_id();
            CanvasEdge {
                id: edge_id,
                canvas_id: self.canvas_id.clone(),
                source_id: pid,
                target_id: card_id.clone(),
                edge_type,
                metadata: json!({}),
                created_at: 0,
            }
        });

        (node, edge, card_id)
    }

    fn handle_step_started(&mut self, step_id: &str, description: &str) -> Vec<CanvasEvent> {
        let (node, edge, card_id) = self.create_child_node(
            CardType::Decomposition,
            json!({ "step_id": step_id, "description": description }),
            CardStatus::Active,
        );
        self.card_stack.push(card_id);
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_model_call_started(&mut self, model: &str, prompt_preview: &str) -> Vec<CanvasEvent> {
        let (node, edge, card_id) = self.create_child_node(
            CardType::Response,
            json!({ "model": model, "prompt_preview": prompt_preview }),
            CardStatus::Active,
        );
        self.pending_model = Some(PendingModel { card_id: card_id.clone() });
        self.card_stack.push(card_id);
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_model_call_completed(&mut self, content: &str, token_count: u32) -> Vec<CanvasEvent> {
        if let Some(pending) = self.pending_model.take() {
            // Pop the model card from the stack
            if self.card_stack.last().map(|s| s.as_str()) == Some(&pending.card_id) {
                self.card_stack.pop();
            }
            vec![CanvasEvent::NodeUpdated {
                node_id: pending.card_id,
                patch: NodePatch {
                    content: Some(json!({ "text": content, "token_count": token_count })),
                    status: Some(CardStatus::Archived),
                    x: None,
                    y: None,
                },
            }]
        } else {
            vec![]
        }
    }

    fn handle_tool_call_started(
        &mut self,
        tool_id: &str,
        input: &serde_json::Value,
    ) -> Vec<CanvasEvent> {
        let (node, edge, card_id) = self.create_child_node(
            CardType::ToolCall,
            json!({ "tool_id": tool_id, "input": input }),
            CardStatus::Active,
        );
        self.pending_tools.insert(tool_id.to_string(), PendingTool { card_id: card_id.clone() });
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_tool_call_completed(
        &mut self,
        tool_id: &str,
        output: &serde_json::Value,
        is_error: bool,
    ) -> Vec<CanvasEvent> {
        if let Some(pending) = self.pending_tools.remove(tool_id) {
            let status = if is_error { CardStatus::DeadEnd } else { CardStatus::Archived };
            vec![CanvasEvent::NodeUpdated {
                node_id: pending.card_id,
                patch: NodePatch {
                    content: Some(json!({ "output": output, "is_error": is_error })),
                    status: Some(status),
                    x: None,
                    y: None,
                },
            }]
        } else {
            vec![]
        }
    }

    fn handle_branch_evaluated(&mut self, condition: &str, result: bool) -> Vec<CanvasEvent> {
        let (node, edge, _card_id) = self.create_child_node(
            CardType::DecisionPoint,
            json!({ "condition": condition, "result": result }),
            CardStatus::Active,
        );
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_path_abandoned(&mut self, reason: &str) -> Vec<CanvasEvent> {
        if let Some(current_id) = self.current_parent().map(|s| s.to_string()) {
            vec![CanvasEvent::NodeStatusChanged {
                node_id: current_id,
                status: CardStatus::DeadEnd,
            }]
        } else {
            // No current parent; create a standalone DeadEnd card
            let (node, edge, _card_id) = self.create_child_node(
                CardType::DeadEnd,
                json!({ "reason": reason }),
                CardStatus::DeadEnd,
            );
            vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
        }
    }

    fn handle_synthesized(&mut self, sources: &[String], result: &str) -> Vec<CanvasEvent> {
        let card_id = self.generate_id();
        let parent_id = self.current_parent().map(|s| s.to_string());

        let (x, y) = if let Some(ref pid) = parent_id {
            self.layout.position_child(pid, &card_id)
        } else {
            self.layout.position_orphan(&card_id)
        };

        let node = CanvasNode {
            id: card_id.clone(),
            canvas_id: self.canvas_id.clone(),
            card_type: CardType::Synthesis,
            x,
            y,
            width: 280.0,
            height: 120.0,
            content: json!({ "sources": sources, "result": result }),
            status: CardStatus::Active,
            created_by: "agent".to_string(),
            created_at: 0,
        };

        let mut events = Vec::new();

        // Edge from parent (if any)
        let parent_edge = parent_id.map(|pid| {
            let edge_id = self.generate_edge_id();
            CanvasEdge {
                id: edge_id,
                canvas_id: self.canvas_id.clone(),
                source_id: pid,
                target_id: card_id.clone(),
                edge_type: EdgeType::Synthesizes,
                metadata: json!({}),
                created_at: 0,
            }
        });

        events.push(CanvasEvent::NodeCreated { node, parent_edge });

        // Create edges from each source to the synthesis card
        for source_id in sources {
            let edge_id = self.generate_edge_id();
            let edge = CanvasEdge {
                id: edge_id,
                canvas_id: self.canvas_id.clone(),
                source_id: source_id.clone(),
                target_id: card_id.clone(),
                edge_type: EdgeType::Synthesizes,
                metadata: json!({}),
                created_at: 0,
            };
            events.push(CanvasEvent::EdgeCreated { edge });
        }

        events
    }

    fn handle_completed(&mut self, result: &str) -> Vec<CanvasEvent> {
        let (node, edge, _card_id) = self.create_child_node(
            CardType::Response,
            json!({ "text": result, "final": true }),
            CardStatus::Archived,
        );
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_failed(&mut self, error: &str) -> Vec<CanvasEvent> {
        let (node, edge, _card_id) = self.create_child_node(
            CardType::DeadEnd,
            json!({ "error": error }),
            CardStatus::DeadEnd,
        );
        vec![CanvasEvent::NodeCreated { node, parent_edge: edge }]
    }

    fn handle_token_delta(&mut self, token: &str) -> Vec<CanvasEvent> {
        if let Some(ref pending) = self.pending_model {
            vec![CanvasEvent::StreamToken {
                node_id: pending.card_id.clone(),
                token: token.to_string(),
            }]
        } else {
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_node_created(event: &CanvasEvent) -> Option<(&CanvasNode, &Option<CanvasEdge>)> {
        match event {
            CanvasEvent::NodeCreated { node, parent_edge } => Some((node, parent_edge)),
            _ => None,
        }
    }

    #[test]
    fn test_new_creates_root_prompt() {
        let (observer, events) = DagObserver::new("canvas-1".into(), "Hello world");
        assert_eq!(events.len(), 1);

        let (node, parent_edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.id, "card-1");
        assert_eq!(node.canvas_id, "canvas-1");
        assert_eq!(node.card_type, CardType::Prompt);
        assert_eq!(node.x, 0.0);
        assert_eq!(node.y, 0.0);
        assert_eq!(node.content["text"], "Hello world");
        assert_eq!(node.status, CardStatus::Active);
        assert!(parent_edge.is_none());

        // Root should be in the card stack
        assert_eq!(observer.card_stack.len(), 1);
        assert_eq!(observer.card_stack[0], "card-1");
    }

    #[test]
    fn test_model_call_creates_response_card() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "gpt-4".into(),
            prompt_preview: "test prompt".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });
        let (node, parent_edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::Response);
        assert_eq!(node.status, CardStatus::Active);
        assert_eq!(node.content["model"], "gpt-4");
        assert_eq!(node.content["prompt_preview"], "test prompt");

        // Should have an edge from root to this card
        let edge = parent_edge.as_ref().unwrap();
        assert_eq!(edge.source_id, "card-1");
        assert_eq!(edge.target_id, node.id);
        assert_eq!(edge.edge_type, EdgeType::ReplyTo);

        // Model card should be in pending
        assert!(obs.pending_model.is_some());
    }

    #[test]
    fn test_model_call_complete_updates_card() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "gpt-4".into(),
            prompt_preview: "test".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });

        let events = obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "Hello! I can help.".into(),
            token_count: 42,
            model: String::new(),
        });

        assert_eq!(events.len(), 1);
        match &events[0] {
            CanvasEvent::NodeUpdated { node_id, patch } => {
                assert_eq!(node_id, "card-2");
                assert_eq!(patch.content.as_ref().unwrap()["text"], "Hello! I can help.");
                assert_eq!(patch.content.as_ref().unwrap()["token_count"], 42);
                assert_eq!(patch.status, Some(CardStatus::Archived));
            }
            other => panic!("Expected NodeUpdated, got {other:?}"),
        }

        // pending_model should be cleared
        assert!(obs.pending_model.is_none());
    }

    #[test]
    fn test_model_call_complete_without_start_is_noop() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "orphan".into(),
            token_count: 0,
            model: String::new(),
        });
        assert!(events.is_empty());
    }

    #[test]
    fn test_tool_call_creates_tool_card() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "search".into(),
            input: json!({"query": "rust"}),
        });

        assert_eq!(events.len(), 1);
        let (node, parent_edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::ToolCall);
        assert_eq!(node.content["tool_id"], "search");
        assert_eq!(node.content["input"]["query"], "rust");

        let edge = parent_edge.as_ref().unwrap();
        assert_eq!(edge.edge_type, EdgeType::ToolIO);

        assert!(obs.pending_tools.contains_key("search"));
    }

    #[test]
    fn test_tool_call_completed_updates_card() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "search".into(),
            input: json!({}),
        });

        let events = obs.observe(&ReasoningEvent::ToolCallCompleted {
            tool_id: "search".into(),
            output: json!({"results": [1, 2, 3]}),
            is_error: false,
        });

        assert_eq!(events.len(), 1);
        match &events[0] {
            CanvasEvent::NodeUpdated { patch, .. } => {
                assert_eq!(patch.status, Some(CardStatus::Archived));
                assert_eq!(patch.content.as_ref().unwrap()["is_error"], false);
            }
            other => panic!("Expected NodeUpdated, got {other:?}"),
        }

        assert!(!obs.pending_tools.contains_key("search"));
    }

    #[test]
    fn test_tool_call_error_marks_dead_end() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        obs.observe(&ReasoningEvent::ToolCallStarted { tool_id: "exec".into(), input: json!({}) });

        let events = obs.observe(&ReasoningEvent::ToolCallCompleted {
            tool_id: "exec".into(),
            output: json!({"error": "command failed"}),
            is_error: true,
        });

        assert_eq!(events.len(), 1);
        match &events[0] {
            CanvasEvent::NodeUpdated { patch, .. } => {
                assert_eq!(patch.status, Some(CardStatus::DeadEnd));
            }
            other => panic!("Expected NodeUpdated, got {other:?}"),
        }
    }

    #[test]
    fn test_tool_call_completed_unknown_tool_is_noop() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::ToolCallCompleted {
            tool_id: "unknown".into(),
            output: json!({}),
            is_error: false,
        });
        assert!(events.is_empty());
    }

    #[test]
    fn test_token_delta_emits_stream_token() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "gpt-4".into(),
            prompt_preview: "test".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });

        let events = obs.observe(&ReasoningEvent::TokenDelta { token: "Hello".into() });

        assert_eq!(events.len(), 1);
        match &events[0] {
            CanvasEvent::StreamToken { node_id, token } => {
                assert_eq!(node_id, "card-2");
                assert_eq!(token, "Hello");
            }
            other => panic!("Expected StreamToken, got {other:?}"),
        }
    }

    #[test]
    fn test_token_delta_without_pending_model_is_noop() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::TokenDelta { token: "orphan".into() });
        assert!(events.is_empty());
    }

    #[test]
    fn test_path_abandoned_marks_dead_end() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        // Push a step first so we have a current parent to mark
        obs.observe(&ReasoningEvent::StepStarted {
            step_id: "s1".into(),
            description: "attempt".into(),
        });

        let events = obs.observe(&ReasoningEvent::PathAbandoned { reason: "no results".into() });

        assert_eq!(events.len(), 1);
        match &events[0] {
            CanvasEvent::NodeStatusChanged { node_id, status } => {
                // The current parent (step card) should be marked DeadEnd
                assert_eq!(status, &CardStatus::DeadEnd);
                // node_id should be the step card
                assert!(!node_id.is_empty());
            }
            other => panic!("Expected NodeStatusChanged, got {other:?}"),
        }
    }

    #[test]
    fn test_branch_evaluated_creates_decision_point() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::BranchEvaluated {
            condition: "has_results".into(),
            result: true,
        });

        assert_eq!(events.len(), 1);
        let (node, _edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::DecisionPoint);
        assert_eq!(node.content["condition"], "has_results");
        assert_eq!(node.content["result"], true);
    }

    #[test]
    fn test_synthesis_creates_synthesis_card_with_edges() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");

        // Create some source cards first via tool calls
        obs.observe(&ReasoningEvent::ToolCallStarted { tool_id: "t1".into(), input: json!({}) });
        obs.observe(&ReasoningEvent::ToolCallStarted { tool_id: "t2".into(), input: json!({}) });

        let source_ids = vec!["card-2".to_string(), "card-4".to_string()];
        let events = obs.observe(&ReasoningEvent::Synthesized {
            sources: source_ids.clone(),
            result: "combined result".into(),
        });

        // Should have: 1 NodeCreated + 2 EdgeCreated (one per source)
        assert_eq!(events.len(), 3);

        let (node, parent_edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::Synthesis);
        assert_eq!(node.content["result"], "combined result");
        // Parent edge should be Synthesizes from root
        assert!(parent_edge.is_some());

        // Check source edges
        for event in &events[1..] {
            match event {
                CanvasEvent::EdgeCreated { edge } => {
                    assert_eq!(edge.edge_type, EdgeType::Synthesizes);
                    assert!(source_ids.contains(&edge.source_id));
                }
                other => panic!("Expected EdgeCreated, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_completed_creates_final_response() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::Completed { result: "final answer".into() });

        assert_eq!(events.len(), 1);
        let (node, edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::Response);
        assert_eq!(node.status, CardStatus::Archived);
        assert_eq!(node.content["text"], "final answer");
        assert_eq!(node.content["final"], true);
        assert!(edge.is_some());
    }

    #[test]
    fn test_failed_creates_dead_end_card() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        let events = obs.observe(&ReasoningEvent::Failed {
            error: "out of tokens".into(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        });

        assert_eq!(events.len(), 1);
        let (node, edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.card_type, CardType::DeadEnd);
        assert_eq!(node.status, CardStatus::DeadEnd);
        assert_eq!(node.content["error"], "out of tokens");
        assert!(edge.is_some());
    }

    #[test]
    fn test_step_started_pushes_to_card_stack() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        assert_eq!(obs.card_stack.len(), 1); // root

        obs.observe(&ReasoningEvent::StepStarted {
            step_id: "s1".into(),
            description: "decompose".into(),
        });

        assert_eq!(obs.card_stack.len(), 2);
    }

    #[test]
    fn test_full_reasoning_flow() {
        let (mut obs, init_events) = DagObserver::new("c1".into(), "What is Rust?");
        assert_eq!(init_events.len(), 1); // root prompt
        let mut total_events: Vec<CanvasEvent> = init_events;

        // ModelCallStarted
        let events = obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "gpt-4".into(),
            prompt_preview: "What is Rust?".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // TokenDelta × 3
        for token in &["Rust", " is", " great"] {
            let events = obs.observe(&ReasoningEvent::TokenDelta { token: token.to_string() });
            assert_eq!(events.len(), 1);
            match &events[0] {
                CanvasEvent::StreamToken { token: t, .. } => assert_eq!(t, token),
                _ => panic!("Expected StreamToken"),
            }
            total_events.extend(events);
        }

        // ModelCallCompleted
        let events = obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "Rust is great".into(),
            token_count: 3,
            model: String::new(),
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // ToolCallStarted
        let events = obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "search".into(),
            input: json!({"query": "rust lang"}),
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // ToolCallCompleted
        let events = obs.observe(&ReasoningEvent::ToolCallCompleted {
            tool_id: "search".into(),
            output: json!({"found": true}),
            is_error: false,
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // Second ModelCallStarted
        let events = obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "gpt-4".into(),
            prompt_preview: "summarize".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // Second ModelCallCompleted
        let events = obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "Final summary".into(),
            token_count: 10,
            model: String::new(),
        });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // Completed
        let events = obs.observe(&ReasoningEvent::Completed { result: "Done".into() });
        assert_eq!(events.len(), 1);
        total_events.extend(events);

        // Count event types
        let node_created =
            total_events.iter().filter(|e| matches!(e, CanvasEvent::NodeCreated { .. })).count();
        let node_updated =
            total_events.iter().filter(|e| matches!(e, CanvasEvent::NodeUpdated { .. })).count();
        let stream_tokens =
            total_events.iter().filter(|e| matches!(e, CanvasEvent::StreamToken { .. })).count();

        // root + model_card + tool_card + model_card_2 + completed = 5 NodeCreated
        assert_eq!(node_created, 5);
        // model_completed + tool_completed + model_completed_2 = 3 NodeUpdated
        assert_eq!(node_updated, 3);
        // 3 TokenDelta → 3 StreamToken
        assert_eq!(stream_tokens, 3);
    }

    #[test]
    fn test_layout_positions_children_below_parent() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");

        // First child
        let events = obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "m".into(),
            prompt_preview: "p".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });
        let (node1, _) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node1.y, Y_SPACING); // depth 1
        let x1 = node1.x;

        // Complete it so we can add a sibling
        obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "done".into(),
            token_count: 1,
            model: String::new(),
        });

        // Second child (sibling)
        let events =
            obs.observe(&ReasoningEvent::ToolCallStarted { tool_id: "t".into(), input: json!({}) });
        let (node2, _) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node2.y, Y_SPACING); // same depth
        assert!(node2.x > x1); // offset horizontally
        assert_eq!(node2.x, x1 + X_SPACING);
    }

    #[test]
    fn test_branching_creates_parallel_cards() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");

        // Multiple tool calls started before any complete
        let events1 = obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "t1".into(),
            input: json!({"a": 1}),
        });
        let events2 = obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "t2".into(),
            input: json!({"b": 2}),
        });
        let events3 = obs.observe(&ReasoningEvent::ToolCallStarted {
            tool_id: "t3".into(),
            input: json!({"c": 3}),
        });

        assert_eq!(events1.len(), 1);
        assert_eq!(events2.len(), 1);
        assert_eq!(events3.len(), 1);

        let (n1, _) = extract_node_created(&events1[0]).unwrap();
        let (n2, _) = extract_node_created(&events2[0]).unwrap();
        let (n3, _) = extract_node_created(&events3[0]).unwrap();

        // All at same depth (children of root)
        assert_eq!(n1.y, n2.y);
        assert_eq!(n2.y, n3.y);

        // Spread horizontally
        assert!(n2.x > n1.x);
        assert!(n3.x > n2.x);

        // All pending
        assert_eq!(obs.pending_tools.len(), 3);
    }

    #[test]
    fn test_nested_steps_create_hierarchy() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");

        // Step 1 at depth 1
        let events = obs.observe(&ReasoningEvent::StepStarted {
            step_id: "s1".into(),
            description: "outer".into(),
        });
        let (n1, _) = extract_node_created(&events[0]).unwrap();
        assert_eq!(n1.y, Y_SPACING);

        // Step 2 at depth 2 (child of step 1)
        let events = obs.observe(&ReasoningEvent::StepStarted {
            step_id: "s2".into(),
            description: "inner".into(),
        });
        let (n2, edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(n2.y, Y_SPACING * 2.0);

        // Edge should go from step 1 to step 2
        let edge = edge.as_ref().unwrap();
        assert_eq!(edge.source_id, n1.id);
        assert_eq!(edge.target_id, n2.id);
        assert_eq!(edge.edge_type, EdgeType::DecomposesTo);

        assert_eq!(obs.card_stack.len(), 3); // root + s1 + s2
    }

    #[test]
    fn test_canvas_id_propagates_to_all_events() {
        let (mut obs, init) = DagObserver::new("my-canvas".into(), "test");

        let (root, _) = extract_node_created(&init[0]).unwrap();
        assert_eq!(root.canvas_id, "my-canvas");

        let events =
            obs.observe(&ReasoningEvent::ToolCallStarted { tool_id: "t".into(), input: json!({}) });
        let (node, edge) = extract_node_created(&events[0]).unwrap();
        assert_eq!(node.canvas_id, "my-canvas");
        assert_eq!(edge.as_ref().unwrap().canvas_id, "my-canvas");
    }

    #[test]
    fn test_model_call_completed_pops_stack() {
        let (mut obs, _) = DagObserver::new("c1".into(), "test");
        assert_eq!(obs.card_stack.len(), 1);

        obs.observe(&ReasoningEvent::ModelCallStarted {
            model: "m".into(),
            prompt_preview: "p".into(),
            tool_result_counts: Default::default(),
            estimated_tokens: None,
        });
        assert_eq!(obs.card_stack.len(), 2);

        obs.observe(&ReasoningEvent::ModelCallCompleted {
            content: "done".into(),
            token_count: 1,
            model: String::new(),
        });
        assert_eq!(obs.card_stack.len(), 1); // popped back to root
    }
}
