use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CardType {
    Prompt,
    Response,
    Artifact,
    Reference,
    Cluster,
    Decomposition,
    ToolCall,
    DecisionPoint,
    Synthesis,
    DeadEnd,
}

impl CardType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Response => "response",
            Self::Artifact => "artifact",
            Self::Reference => "reference",
            Self::Cluster => "cluster",
            Self::Decomposition => "decomposition",
            Self::ToolCall => "tool_call",
            Self::DecisionPoint => "decision_point",
            Self::Synthesis => "synthesis",
            Self::DeadEnd => "dead_end",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "prompt" => Some(Self::Prompt),
            "response" => Some(Self::Response),
            "artifact" => Some(Self::Artifact),
            "reference" => Some(Self::Reference),
            "cluster" => Some(Self::Cluster),
            "decomposition" => Some(Self::Decomposition),
            "tool_call" => Some(Self::ToolCall),
            "decision_point" => Some(Self::DecisionPoint),
            "synthesis" => Some(Self::Synthesis),
            "dead_end" => Some(Self::DeadEnd),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    Active,
    DeadEnd,
    Archived,
}

impl CardStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::DeadEnd => "dead_end",
            Self::Archived => "archived",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "dead_end" => Some(Self::DeadEnd),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    ReplyTo,
    References,
    Contradicts,
    Evolves,
    DecomposesTo,
    ToolIO,
    Synthesizes,
    Delegation,
    ContextShare,
    ArtifactPass,
    FeedbackLoop,
    BlockedBy,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReplyTo => "reply_to",
            Self::References => "references",
            Self::Contradicts => "contradicts",
            Self::Evolves => "evolves",
            Self::DecomposesTo => "decomposes_to",
            Self::ToolIO => "tool_io",
            Self::Synthesizes => "synthesizes",
            Self::Delegation => "delegation",
            Self::ContextShare => "context_share",
            Self::ArtifactPass => "artifact_pass",
            Self::FeedbackLoop => "feedback_loop",
            Self::BlockedBy => "blocked_by",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "reply_to" => Some(Self::ReplyTo),
            "references" => Some(Self::References),
            "contradicts" => Some(Self::Contradicts),
            "evolves" => Some(Self::Evolves),
            "decomposes_to" => Some(Self::DecomposesTo),
            "tool_io" => Some(Self::ToolIO),
            "synthesizes" => Some(Self::Synthesizes),
            "delegation" => Some(Self::Delegation),
            "context_share" => Some(Self::ContextShare),
            "artifact_pass" => Some(Self::ArtifactPass),
            "feedback_loop" => Some(Self::FeedbackLoop),
            "blocked_by" => Some(Self::BlockedBy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasNode {
    pub id: String,
    pub canvas_id: String,
    pub card_type: CardType,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub content: serde_json::Value,
    pub status: CardStatus,
    pub created_by: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasEdge {
    pub id: String,
    pub canvas_id: String,
    pub source_id: String,
    pub target_id: String,
    pub edge_type: EdgeType,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}
