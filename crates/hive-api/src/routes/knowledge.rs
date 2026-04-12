use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{clamp_limit, kg_error, open_kg, AppState};
use hive_classification::DataClass;
use hive_knowledge::{Edge, NewNode, Node};

// ── Request / response types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct KgListNodesQuery {
    node_type: Option<String>,
    data_class: Option<DataClass>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgSearchQuery {
    q: String,
    data_class: Option<DataClass>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgCreateNodeRequest {
    node_type: String,
    name: String,
    data_class: Option<DataClass>,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgCreateEdgeRequest {
    source_id: i64,
    target_id: i64,
    edge_type: String,
    weight: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgIdResponse {
    id: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgStatsResponse {
    node_count: i64,
    edge_count: i64,
    nodes_by_type: Vec<KgTypeCount>,
    edges_by_type: Vec<KgTypeCount>,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgTypeCount {
    name: String,
    count: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgNodeWithEdges {
    #[serde(flatten)]
    node: Node,
    edges: Vec<Edge>,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgNeighborhoodResponse {
    #[serde(flatten)]
    node: Node,
    edges: Vec<Edge>,
    neighbors: Vec<Node>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgNeighborsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgVectorSearchQuery {
    q: String,
    data_class: Option<DataClass>,
    limit: Option<usize>,
    model: Option<String>,
    /// Maximum L2 distance threshold.  Results farther than this are discarded.
    /// Defaults to `DEFAULT_VECTOR_SEARCH_MAX_DISTANCE` when omitted.
    max_distance: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgVectorSearchResultItem {
    #[serde(flatten)]
    node: Node,
    distance: f32,
}

/// A workspace file search result: the file path, a text snippet, and the
/// node ID (for further navigation).
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSearchResult {
    pub path: String,
    pub snippet: String,
    pub node_id: i64,
}

/// Workspace semantic search result with distance score.
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSemanticSearchResult {
    pub path: String,
    pub snippet: String,
    pub node_id: i64,
    pub distance: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KgUpdateNodeRequest {
    name: Option<String>,
    content: Option<String>,
    data_class: Option<DataClass>,
}

#[derive(Debug, Serialize)]
pub(crate) struct KgEmbeddingModelInfo {
    model_id: String,
    dimensions: usize,
}

// ── Handlers ─────────────────────────────────────────────────────────────

pub(crate) async fn kg_list_nodes(
    State(state): State<AppState>,
    Query(params): Query<KgListNodesQuery>,
) -> Result<Json<Vec<Node>>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 50);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        graph
            .list_nodes(params.node_type.as_deref(), params.data_class, limit)
            .map(Json)
            .map_err(kg_error)
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_create_node(
    State(state): State<AppState>,
    Json(body): Json<KgCreateNodeRequest>,
) -> Result<(StatusCode, Json<KgIdResponse>), (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let id = graph
            .insert_node(&NewNode {
                node_type: body.node_type,
                name: body.name,
                data_class: body.data_class.unwrap_or(DataClass::Internal),
                content: body.content,
            })
            .map_err(kg_error)?;
        Ok((StatusCode::CREATED, Json(KgIdResponse { id })))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_get_node(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
) -> Result<Json<KgNodeWithEdges>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let node = graph
            .get_node(node_id)
            .map_err(kg_error)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "node not found".to_string()))?;
        let edges = graph.get_edges_for_node(node_id).map_err(kg_error)?;
        Ok(Json(KgNodeWithEdges { node, edges }))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_update_node(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
    Json(body): Json<KgUpdateNodeRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let _node = graph
            .get_node(node_id)
            .map_err(kg_error)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "node not found".to_string()))?;
        if let Some(name) = &body.name {
            graph.update_node_name(node_id, name).map_err(kg_error)?;
        }
        if let Some(content) = &body.content {
            graph.update_node_content(node_id, content).map_err(kg_error)?;
        }
        if let Some(data_class) = body.data_class {
            graph.update_node_data_class(node_id, data_class).map_err(kg_error)?;
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_get_neighbors(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
    Query(params): Query<KgNeighborsQuery>,
) -> Result<Json<KgNeighborhoodResponse>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 20);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let neighborhood = graph.get_node_with_neighbors(node_id, limit).map_err(kg_error)?;
        Ok(Json(KgNeighborhoodResponse {
            node: neighborhood.node,
            edges: neighborhood.edges,
            neighbors: neighborhood.neighbors,
        }))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_delete_node(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let removed = graph.remove_node(node_id).map_err(kg_error)?;
        if removed {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err((StatusCode::NOT_FOUND, "node not found".to_string()))
        }
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_get_node_edges(
    State(state): State<AppState>,
    Path(node_id): Path<i64>,
) -> Result<Json<Vec<Edge>>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        graph.get_edges_for_node(node_id).map(Json).map_err(kg_error)
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_create_edge(
    State(state): State<AppState>,
    Json(body): Json<KgCreateEdgeRequest>,
) -> Result<(StatusCode, Json<KgIdResponse>), (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let id = graph
            .insert_edge(
                body.source_id,
                body.target_id,
                &body.edge_type,
                body.weight.unwrap_or(1.0),
            )
            .map_err(kg_error)?;
        Ok((StatusCode::CREATED, Json(KgIdResponse { id })))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_delete_edge(
    State(state): State<AppState>,
    Path(edge_id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let removed = graph.remove_edge(edge_id).map_err(kg_error)?;
        if removed {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err((StatusCode::NOT_FOUND, "edge not found".to_string()))
        }
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_search(
    State(state): State<AppState>,
    Query(params): Query<KgSearchQuery>,
) -> Result<Json<Vec<Node>>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 20);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let results = match params.data_class {
            Some(dc) => graph.search_text_filtered(&params.q, dc, limit).map_err(kg_error)?,
            None => graph
                .search_text_filtered(&params.q, DataClass::Restricted, limit)
                .map_err(kg_error)?,
        };
        let nodes: Vec<Node> = results
            .into_iter()
            .map(|sr| Node {
                id: sr.id,
                node_type: sr.node_type,
                name: sr.name,
                data_class: sr.data_class,
                content: sr.content,
                created_at: sr.created_at,
                updated_at: sr.updated_at,
            })
            .collect();
        Ok(Json(nodes))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_vector_search(
    State(state): State<AppState>,
    Query(params): Query<KgVectorSearchQuery>,
) -> Result<Json<Vec<KgVectorSearchResultItem>>, (StatusCode, String)> {
    let rt = state
        .runtime_manager
        .as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "no runtime manager".to_string()))?
        .clone();
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 20);
    let max_class = params.data_class.unwrap_or(DataClass::Restricted);
    let query_text = params.q.clone();
    let max_distance =
        Some(params.max_distance.unwrap_or(hive_knowledge::DEFAULT_VECTOR_SEARCH_MAX_DISTANCE));

    let model_id =
        params.model.as_deref().unwrap_or(hive_inference::defaults::DEFAULT_EMBEDDING_MODEL_ID);
    let embed_model_id = model_id.to_string();
    let search_model_id = model_id.to_string();

    tokio::task::spawn_blocking(move || {
        let embedding = rt
            .embed(&embed_model_id, &query_text)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("embedding failed: {e}")))?;
        let graph = open_kg(&path)?;
        let results = graph
            .search_similar_filtered(
                &embedding,
                &search_model_id,
                max_class,
                None,
                max_distance,
                limit,
            )
            .map_err(kg_error)?;

        let items: Vec<KgVectorSearchResultItem> = results
            .into_iter()
            .filter_map(|vr| {
                graph
                    .get_node(vr.id)
                    .ok()
                    .flatten()
                    .map(|node| KgVectorSearchResultItem { node, distance: vr.distance as f32 })
            })
            .collect();
        Ok(Json(items))
    })
    .await
    .map_err(kg_error)?
}

/// Workspace-scoped keyword (FTS) search.
///
/// Searches `workspace_file` and `file_chunk` nodes only, deduplicates by
/// parent file, and returns file paths with text snippets.
pub(crate) async fn workspace_search(
    State(state): State<AppState>,
    Query(params): Query<KgSearchQuery>,
) -> Result<Json<Vec<WorkspaceSearchResult>>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 20);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let node_types: &[&str] = &["workspace_file", "file_chunk"];
        let max_class = params.data_class.unwrap_or(DataClass::Restricted);
        let results = graph
            .search_text_filtered_by_type(&params.q, max_class, Some(node_types), limit * 3)
            .map_err(kg_error)?;

        let mut seen_paths = std::collections::HashSet::new();
        let mut items = Vec::new();

        for sr in results {
            // For file_chunk nodes, `name` is the file path.
            // For workspace_file nodes, `name` is also the file path.
            let file_path = sr.name.clone();
            if !seen_paths.insert(file_path.clone()) {
                continue;
            }
            let snippet = sr.content.as_deref().unwrap_or("").chars().take(200).collect::<String>();
            items.push(WorkspaceSearchResult { path: file_path, snippet, node_id: sr.id });
            if items.len() >= limit {
                break;
            }
        }
        Ok(Json(items))
    })
    .await
    .map_err(kg_error)?
}

/// Workspace-scoped semantic (vector) search.
pub(crate) async fn workspace_semantic_search(
    State(state): State<AppState>,
    Query(params): Query<KgVectorSearchQuery>,
) -> Result<Json<Vec<WorkspaceSemanticSearchResult>>, (StatusCode, String)> {
    let rt = state
        .runtime_manager
        .as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "no runtime manager".to_string()))?
        .clone();
    let path = Arc::clone(&state.knowledge_graph_path);
    let limit = clamp_limit(params.limit, 20);
    let max_class = params.data_class.unwrap_or(DataClass::Restricted);
    let query_text = params.q.clone();
    let max_distance =
        Some(params.max_distance.unwrap_or(hive_knowledge::DEFAULT_VECTOR_SEARCH_MAX_DISTANCE));

    let model_id =
        params.model.as_deref().unwrap_or(hive_inference::defaults::DEFAULT_EMBEDDING_MODEL_ID);
    let embed_model_id = model_id.to_string();
    let search_model_id = model_id.to_string();

    tokio::task::spawn_blocking(move || {
        let embedding = rt
            .embed(&embed_model_id, &query_text)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("embedding failed: {e}")))?;
        let graph = open_kg(&path)?;
        let node_types: &[&str] = &["workspace_file", "file_chunk"];
        let results = graph
            .search_similar_filtered(
                &embedding,
                &search_model_id,
                max_class,
                Some(node_types),
                max_distance,
                limit * 3,
            )
            .map_err(kg_error)?;

        let mut seen_paths = std::collections::HashSet::new();
        let mut items = Vec::new();

        for vr in results {
            let node = match graph.get_node(vr.id).ok().flatten() {
                Some(n) => n,
                None => continue,
            };
            let file_path = node.name.clone();
            if !seen_paths.insert(file_path.clone()) {
                continue;
            }
            let snippet =
                node.content.as_deref().unwrap_or("").chars().take(200).collect::<String>();
            items.push(WorkspaceSemanticSearchResult {
                path: file_path,
                snippet,
                node_id: vr.id,
                distance: vr.distance as f32,
            });
            if items.len() >= limit {
                break;
            }
        }
        Ok(Json(items))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_stats(
    State(state): State<AppState>,
) -> Result<Json<KgStatsResponse>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let node_count = graph.node_count().map_err(kg_error)?;
        let edge_count = graph.edge_count().map_err(kg_error)?;
        let nodes_by_type = graph
            .node_counts_by_type()
            .map_err(kg_error)?
            .into_iter()
            .map(|(name, count)| KgTypeCount { name, count })
            .collect();
        let edges_by_type = graph
            .edge_counts_by_type()
            .map_err(kg_error)?
            .into_iter()
            .map(|(name, count)| KgTypeCount { name, count })
            .collect();
        Ok(Json(KgStatsResponse { node_count, edge_count, nodes_by_type, edges_by_type }))
    })
    .await
    .map_err(kg_error)?
}

pub(crate) async fn kg_list_embedding_models(
    State(state): State<AppState>,
) -> Result<Json<Vec<KgEmbeddingModelInfo>>, (StatusCode, String)> {
    let path = Arc::clone(&state.knowledge_graph_path);
    tokio::task::spawn_blocking(move || {
        let graph = open_kg(&path)?;
        let models = graph.list_embedding_models().map_err(kg_error)?;
        let items = models
            .into_iter()
            .map(|(model_id, dimensions)| KgEmbeddingModelInfo { model_id, dimensions })
            .collect();
        Ok(Json(items))
    })
    .await
    .map_err(kg_error)?
}
