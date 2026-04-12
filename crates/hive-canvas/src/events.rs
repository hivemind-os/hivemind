use serde::{Deserialize, Serialize};

use crate::layout::LayoutPosition;
use crate::types::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasEvent {
    NodeCreated {
        node: CanvasNode,
        parent_edge: Option<CanvasEdge>,
    },
    NodeUpdated {
        node_id: String,
        patch: NodePatch,
    },
    NodeStatusChanged {
        node_id: String,
        status: CardStatus,
    },
    EdgeCreated {
        edge: CanvasEdge,
    },
    StreamToken {
        node_id: String,
        token: String,
    },
    LayoutProposal {
        proposal_id: String,
        algorithm: String,
        positions: Vec<LayoutPosition>,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodePatch {
    pub content: Option<serde_json::Value>,
    pub status: Option<CardStatus>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}
