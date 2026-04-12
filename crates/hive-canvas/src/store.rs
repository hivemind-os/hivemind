use crate::error::CanvasError;
use crate::types::*;

pub trait CanvasStore: Send + Sync {
    // CRUD
    fn insert_node(&self, node: &CanvasNode) -> Result<(), CanvasError>;
    fn insert_edge(&self, edge: &CanvasEdge) -> Result<(), CanvasError>;
    fn update_node_position(&self, node_id: &str, x: f64, y: f64) -> Result<(), CanvasError>;
    fn update_node_content(
        &self,
        node_id: &str,
        content: &serde_json::Value,
    ) -> Result<(), CanvasError>;
    fn update_node_status(&self, node_id: &str, status: &CardStatus) -> Result<(), CanvasError>;
    fn delete_node(&self, node_id: &str) -> Result<(), CanvasError>;
    fn get_node(&self, node_id: &str) -> Result<Option<CanvasNode>, CanvasError>;
    fn get_edges_from(&self, node_id: &str) -> Result<Vec<CanvasEdge>, CanvasError>;
    fn get_edges_to(&self, node_id: &str) -> Result<Vec<CanvasEdge>, CanvasError>;

    // Spatial queries
    fn query_viewport(
        &self,
        canvas_id: &str,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Result<Vec<CanvasNode>, CanvasError>;
    fn query_radius(
        &self,
        canvas_id: &str,
        cx: f64,
        cy: f64,
        radius: f64,
    ) -> Result<Vec<CanvasNode>, CanvasError>;
    fn query_nearest(
        &self,
        canvas_id: &str,
        cx: f64,
        cy: f64,
        k: usize,
    ) -> Result<Vec<CanvasNode>, CanvasError>;

    // Graph traversal
    fn bfs(&self, start_id: &str, max_depth: usize) -> Result<Vec<CanvasNode>, CanvasError>;
    fn connected_component(&self, node_id: &str) -> Result<Vec<CanvasNode>, CanvasError>;

    // Bulk
    fn get_all_nodes(&self, canvas_id: &str) -> Result<Vec<CanvasNode>, CanvasError>;
    fn get_all_edges(&self, canvas_id: &str) -> Result<Vec<CanvasEdge>, CanvasError>;
}
