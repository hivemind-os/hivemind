use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use crate::AppState;

#[derive(Serialize)]
pub(crate) struct EntityNodeResponse {
    pub entity_id: String,
    pub entity_type: String,
    pub parent_ref: Option<String>,
    pub label: String,
    pub created_at: i64,
}

impl From<hive_core::EntityNode> for EntityNodeResponse {
    fn from(n: hive_core::EntityNode) -> Self {
        Self {
            entity_id: n.entity_id,
            entity_type: format!("{:?}", n.entity_type).to_lowercase(),
            parent_ref: n.parent_ref,
            label: n.label,
            created_at: n.created_at,
        }
    }
}

/// GET /api/v1/entity-graph — list all entities (roots only by default)
pub(crate) async fn api_list_entities(
    State(state): State<AppState>,
) -> Json<Vec<EntityNodeResponse>> {
    let nodes = state.entity_graph.roots();
    Json(nodes.into_iter().map(EntityNodeResponse::from).collect())
}

/// GET /api/v1/entity-graph/:entity_type/:entity_id — get a single entity
pub(crate) async fn api_get_entity(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> Json<Option<EntityNodeResponse>> {
    let full_id = format!("{entity_type}/{entity_id}");
    Json(state.entity_graph.get(&full_id).map(EntityNodeResponse::from))
}

/// GET /api/v1/entity-graph/:entity_type/:entity_id/children — direct children
pub(crate) async fn api_entity_children(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> Json<Vec<EntityNodeResponse>> {
    let full_id = format!("{entity_type}/{entity_id}");
    let nodes = state.entity_graph.children(&full_id);
    Json(nodes.into_iter().map(EntityNodeResponse::from).collect())
}

/// GET /api/v1/entity-graph/:entity_type/:entity_id/ancestors — walk to root
pub(crate) async fn api_entity_ancestors(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> Json<Vec<EntityNodeResponse>> {
    let full_id = format!("{entity_type}/{entity_id}");
    let nodes = state.entity_graph.ancestors(&full_id);
    Json(nodes.into_iter().map(EntityNodeResponse::from).collect())
}

/// GET /api/v1/entity-graph/:entity_type/:entity_id/descendants — recursive children
pub(crate) async fn api_entity_descendants(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> Json<Vec<EntityNodeResponse>> {
    let full_id = format!("{entity_type}/{entity_id}");
    let nodes = state.entity_graph.descendants(&full_id);
    Json(nodes.into_iter().map(EntityNodeResponse::from).collect())
}
