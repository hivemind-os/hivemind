//! Layout engines for spatial canvas.
//!
//! Three algorithms for computing card positions:
//! - **Tree**: Hierarchical top-down layout following parent-child edges
//! - **ForceDirected**: Spring/repulsion physics run to convergence
//! - **Radial**: Root at center, children in concentric rings by graph depth

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::types::{CanvasEdge, CanvasNode, CardStatus, CardType};

/// A proposed position for a single node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutPosition {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
}

/// Available layout algorithms.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlgorithm {
    Tree,
    ForceDirected,
    Radial,
}

impl LayoutAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::ForceDirected => "force_directed",
            Self::Radial => "radial",
        }
    }
}

/// Compute positions for the given nodes/edges using the specified algorithm.
pub fn compute_layout(
    algorithm: &LayoutAlgorithm,
    nodes: &[CanvasNode],
    edges: &[CanvasEdge],
) -> Vec<LayoutPosition> {
    let active: Vec<&CanvasNode> = nodes
        .iter()
        .filter(|n| n.status == CardStatus::Active && n.card_type != CardType::Cluster)
        .collect();

    if active.is_empty() {
        return Vec::new();
    }

    match algorithm {
        LayoutAlgorithm::Tree => tree_layout(&active, edges),
        LayoutAlgorithm::ForceDirected => force_directed_layout(&active, edges),
        LayoutAlgorithm::Radial => radial_layout(&active, edges),
    }
}

// ---------------------------------------------------------------------------
// Graph helpers
// ---------------------------------------------------------------------------

/// Build adjacency: parent → children, and track which nodes have parents.
fn build_graph(
    nodes: &[&CanvasNode],
    edges: &[CanvasEdge],
) -> (HashMap<String, Vec<String>>, HashSet<String>) {
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut has_parent: HashSet<String> = HashSet::new();

    for edge in edges {
        if node_ids.contains(edge.source_id.as_str()) && node_ids.contains(edge.target_id.as_str())
        {
            children.entry(edge.source_id.clone()).or_default().push(edge.target_id.clone());
            has_parent.insert(edge.target_id.clone());
        }
    }

    (children, has_parent)
}

/// Find root nodes (nodes with no incoming edges from the active set).
fn find_roots(nodes: &[&CanvasNode], has_parent: &HashSet<String>) -> Vec<String> {
    let mut roots: Vec<String> =
        nodes.iter().filter(|n| !has_parent.contains(&n.id)).map(|n| n.id.clone()).collect();
    roots.sort(); // deterministic order
    if roots.is_empty() && !nodes.is_empty() {
        // Cycle: pick the first node
        roots.push(nodes[0].id.clone());
    }
    roots
}

// ---------------------------------------------------------------------------
// Tree Layout
// ---------------------------------------------------------------------------

const TREE_X_SPACING: f64 = 320.0;
const TREE_Y_SPACING: f64 = 200.0;

fn tree_layout(nodes: &[&CanvasNode], edges: &[CanvasEdge]) -> Vec<LayoutPosition> {
    let (children_map, has_parent) = build_graph(nodes, edges);
    let roots = find_roots(nodes, &has_parent);

    let mut positions: HashMap<String, (f64, f64)> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut next_x_at_depth: HashMap<usize, f64> = HashMap::new();

    // BFS from each root
    for (root_idx, root_id) in roots.iter().enumerate() {
        let root_x_offset = root_idx as f64 * TREE_X_SPACING * 3.0;
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((root_id.clone(), 0));
        visited.insert(root_id.clone());

        while let Some((node_id, depth)) = queue.pop_front() {
            let x = next_x_at_depth.entry(depth).or_insert(0.0);
            let pos = (*x + root_x_offset, depth as f64 * TREE_Y_SPACING);
            *x += TREE_X_SPACING;
            positions.insert(node_id.clone(), pos);

            if let Some(kids) = children_map.get(&node_id) {
                for kid in kids {
                    if visited.insert(kid.clone()) {
                        queue.push_back((kid.clone(), depth + 1));
                    }
                }
            }
        }
    }

    // Place any unvisited nodes (disconnected) in a row below
    let max_depth = positions.values().map(|(_, y)| *y).fold(0.0f64, f64::max);
    let mut orphan_x = 0.0;
    for node in nodes {
        if !positions.contains_key(&node.id) {
            positions.insert(node.id.clone(), (orphan_x, max_depth + TREE_Y_SPACING));
            orphan_x += TREE_X_SPACING;
        }
    }

    positions.into_iter().map(|(node_id, (x, y))| LayoutPosition { node_id, x, y }).collect()
}

// ---------------------------------------------------------------------------
// Force-Directed Layout
// ---------------------------------------------------------------------------

const FORCE_ITERATIONS: usize = 200;
const FORCE_REPULSION: f64 = 8000.0;
const FORCE_ATTRACTION: f64 = 0.008;
const FORCE_REST_LENGTH: f64 = 250.0;
const FORCE_DAMPING: f64 = 0.85;
const FORCE_MAX_VELOCITY: f64 = 30.0;
const FORCE_CENTERING: f64 = 0.001;

fn force_directed_layout(nodes: &[&CanvasNode], edges: &[CanvasEdge]) -> Vec<LayoutPosition> {
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    // Initialize positions from current node positions
    let mut pos: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x, n.y)).collect();
    let mut vel: Vec<(f64, f64)> = vec![(0.0, 0.0); nodes.len()];

    let id_to_idx: HashMap<&str, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();

    // Filter edges to those between active nodes
    let active_edges: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|e| {
            if node_ids.contains(e.source_id.as_str()) && node_ids.contains(e.target_id.as_str()) {
                Some((id_to_idx[e.source_id.as_str()], id_to_idx[e.target_id.as_str()]))
            } else {
                None
            }
        })
        .collect();

    let n = nodes.len();

    for _iter in 0..FORCE_ITERATIONS {
        let mut forces: Vec<(f64, f64)> = vec![(0.0, 0.0); n];

        // Repulsive forces between all pairs
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let dist_sq = dx * dx + dy * dy;
                let dist = dist_sq.sqrt().max(1.0);
                let force = FORCE_REPULSION / dist_sq.max(1.0);
                let fx = force * dx / dist;
                let fy = force * dy / dist;
                forces[i].0 += fx;
                forces[i].1 += fy;
                forces[j].0 -= fx;
                forces[j].1 -= fy;
            }
        }

        // Attractive forces along edges
        for &(src, tgt) in &active_edges {
            let dx = pos[tgt].0 - pos[src].0;
            let dy = pos[tgt].1 - pos[src].1;
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let force = FORCE_ATTRACTION * (dist - FORCE_REST_LENGTH);
            let fx = force * dx / dist;
            let fy = force * dy / dist;
            forces[src].0 += fx;
            forces[src].1 += fy;
            forces[tgt].0 -= fx;
            forces[tgt].1 -= fy;
        }

        // Centering force
        for i in 0..n {
            forces[i].0 -= FORCE_CENTERING * pos[i].0;
            forces[i].1 -= FORCE_CENTERING * pos[i].1;
        }

        // Apply forces
        for i in 0..n {
            vel[i].0 = (vel[i].0 + forces[i].0) * FORCE_DAMPING;
            vel[i].1 = (vel[i].1 + forces[i].1) * FORCE_DAMPING;
            vel[i].0 = vel[i].0.clamp(-FORCE_MAX_VELOCITY, FORCE_MAX_VELOCITY);
            vel[i].1 = vel[i].1.clamp(-FORCE_MAX_VELOCITY, FORCE_MAX_VELOCITY);
            pos[i].0 += vel[i].0;
            pos[i].1 += vel[i].1;
        }
    }

    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| LayoutPosition { node_id: n.id.clone(), x: pos[i].0, y: pos[i].1 })
        .collect()
}

// ---------------------------------------------------------------------------
// Radial Layout
// ---------------------------------------------------------------------------

const RADIAL_RING_SPACING: f64 = 250.0;

fn radial_layout(nodes: &[&CanvasNode], edges: &[CanvasEdge]) -> Vec<LayoutPosition> {
    let (children_map, has_parent) = build_graph(nodes, edges);
    let roots = find_roots(nodes, &has_parent);

    let mut positions: HashMap<String, (f64, f64)> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();

    // BFS to assign depth
    let mut depth_buckets: HashMap<usize, Vec<String>> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for root_id in &roots {
        if visited.insert(root_id.clone()) {
            queue.push_back((root_id.clone(), 0));
        }
    }

    while let Some((node_id, depth)) = queue.pop_front() {
        depth_buckets.entry(depth).or_default().push(node_id.clone());

        if let Some(kids) = children_map.get(&node_id) {
            for kid in kids {
                if visited.insert(kid.clone()) {
                    queue.push_back((kid.clone(), depth + 1));
                }
            }
        }
    }

    // Place unvisited nodes at max_depth + 1
    let max_depth = depth_buckets.keys().copied().max().unwrap_or(0);
    for node in nodes {
        if !visited.contains(&node.id) {
            depth_buckets.entry(max_depth + 1).or_default().push(node.id.clone());
        }
    }

    // Roots at center (0,0) spread slightly if multiple
    if let Some(root_nodes) = depth_buckets.get(&0) {
        let count = root_nodes.len();
        for (i, node_id) in root_nodes.iter().enumerate() {
            if count == 1 {
                positions.insert(node_id.clone(), (0.0, 0.0));
            } else {
                let angle = 2.0 * std::f64::consts::PI * i as f64 / count as f64;
                let r = RADIAL_RING_SPACING * 0.3;
                positions.insert(node_id.clone(), (r * angle.cos(), r * angle.sin()));
            }
        }
    }

    // Each subsequent depth → ring at increasing radius
    let max_d = depth_buckets.keys().copied().max().unwrap_or(0);
    for depth in 1..=max_d {
        if let Some(ring_nodes) = depth_buckets.get(&depth) {
            let radius = depth as f64 * RADIAL_RING_SPACING;
            let count = ring_nodes.len();
            for (i, node_id) in ring_nodes.iter().enumerate() {
                let angle = 2.0 * std::f64::consts::PI * i as f64 / count as f64
                    - std::f64::consts::FRAC_PI_2; // Start from top
                positions.insert(node_id.clone(), (radius * angle.cos(), radius * angle.sin()));
            }
        }
    }

    positions.into_iter().map(|(node_id, (x, y))| LayoutPosition { node_id, x, y }).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_node(id: &str, x: f64, y: f64) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            canvas_id: "test".to_string(),
            card_type: CardType::Prompt,
            x,
            y,
            width: 280.0,
            height: 120.0,
            content: json!({"text": id}),
            status: CardStatus::Active,
            created_by: "user".to_string(),
            created_at: 0,
        }
    }

    fn make_edge(source: &str, target: &str) -> CanvasEdge {
        CanvasEdge {
            id: format!("{source}->{target}"),
            canvas_id: "test".to_string(),
            source_id: source.to_string(),
            target_id: target.to_string(),
            edge_type: crate::types::EdgeType::ReplyTo,
            metadata: json!({}),
            created_at: 0,
        }
    }

    // --- Tree layout tests ---

    #[test]
    fn tree_layout_single_node() {
        let nodes = vec![make_node("a", 0.0, 0.0)];
        let result = compute_layout(&LayoutAlgorithm::Tree, &nodes, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].node_id, "a");
    }

    #[test]
    fn tree_layout_linear_chain() {
        let nodes =
            vec![make_node("a", 0.0, 0.0), make_node("b", 0.0, 0.0), make_node("c", 0.0, 0.0)];
        let edges = vec![make_edge("a", "b"), make_edge("b", "c")];
        let result = compute_layout(&LayoutAlgorithm::Tree, &nodes, &edges);
        assert_eq!(result.len(), 3);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        // a is root (depth 0), b at depth 1, c at depth 2
        assert!(pos["a"].1 < pos["b"].1);
        assert!(pos["b"].1 < pos["c"].1);
    }

    #[test]
    fn tree_layout_siblings_spread_horizontally() {
        let nodes = vec![
            make_node("root", 0.0, 0.0),
            make_node("child1", 0.0, 0.0),
            make_node("child2", 0.0, 0.0),
        ];
        let edges = vec![make_edge("root", "child1"), make_edge("root", "child2")];
        let result = compute_layout(&LayoutAlgorithm::Tree, &nodes, &edges);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        // Siblings at same depth but different x
        assert!((pos["child1"].1 - pos["child2"].1).abs() < 1.0);
        assert!((pos["child1"].0 - pos["child2"].0).abs() > 100.0);
    }

    #[test]
    fn tree_layout_filters_inactive_and_cluster_nodes() {
        let mut cluster = make_node("cl", 0.0, 0.0);
        cluster.card_type = CardType::Cluster;
        let mut archived = make_node("arch", 0.0, 0.0);
        archived.status = CardStatus::Archived;
        let active = make_node("a", 0.0, 0.0);

        let nodes = vec![cluster, archived, active];
        let result = compute_layout(&LayoutAlgorithm::Tree, &nodes, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].node_id, "a");
    }

    // --- Force-directed tests ---

    #[test]
    fn force_layout_separates_disconnected_nodes() {
        let nodes = vec![make_node("a", 1.0, 0.0), make_node("b", -1.0, 0.0)];
        let result = compute_layout(&LayoutAlgorithm::ForceDirected, &nodes, &[]);
        assert_eq!(result.len(), 2);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        // Repulsion should push them apart
        let dx = pos["a"].0 - pos["b"].0;
        let dy = pos["a"].1 - pos["b"].1;
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(dist > 50.0, "nodes should be pushed apart, got dist={dist}");
    }

    #[test]
    fn force_layout_connected_nodes_near_rest_length() {
        let nodes = vec![make_node("a", -500.0, 0.0), make_node("b", 500.0, 0.0)];
        let edges = vec![make_edge("a", "b")];
        let result = compute_layout(&LayoutAlgorithm::ForceDirected, &nodes, &edges);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        let dx = pos["a"].0 - pos["b"].0;
        let dy = pos["a"].1 - pos["b"].1;
        let dist = (dx * dx + dy * dy).sqrt();
        // Should settle near rest length (250) ± tolerance
        assert!(
            dist > 100.0 && dist < 500.0,
            "connected nodes should be near rest length, got dist={dist}"
        );
    }

    // --- Radial layout tests ---

    #[test]
    fn radial_layout_single_root_at_center() {
        let nodes = vec![make_node("root", 100.0, 200.0)];
        let result = compute_layout(&LayoutAlgorithm::Radial, &nodes, &[]);
        assert_eq!(result.len(), 1);
        assert!((result[0].x).abs() < 1.0);
        assert!((result[0].y).abs() < 1.0);
    }

    #[test]
    fn radial_layout_children_on_ring() {
        let nodes = vec![
            make_node("root", 0.0, 0.0),
            make_node("c1", 0.0, 0.0),
            make_node("c2", 0.0, 0.0),
            make_node("c3", 0.0, 0.0),
        ];
        let edges = vec![make_edge("root", "c1"), make_edge("root", "c2"), make_edge("root", "c3")];
        let result = compute_layout(&LayoutAlgorithm::Radial, &nodes, &edges);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        // Root at/near center
        let root_r = (pos["root"].0.powi(2) + pos["root"].1.powi(2)).sqrt();
        assert!(root_r < 10.0, "root should be at center, got r={root_r}");

        // Children at same radius (ring 1)
        for id in &["c1", "c2", "c3"] {
            let r = (pos[*id].0.powi(2) + pos[*id].1.powi(2)).sqrt();
            assert!(
                (r - RADIAL_RING_SPACING).abs() < 1.0,
                "child {id} should be at ring 1 radius, got r={r}"
            );
        }
    }

    #[test]
    fn radial_layout_two_depth_levels() {
        let nodes = vec![
            make_node("root", 0.0, 0.0),
            make_node("mid", 0.0, 0.0),
            make_node("leaf", 0.0, 0.0),
        ];
        let edges = vec![make_edge("root", "mid"), make_edge("mid", "leaf")];
        let result = compute_layout(&LayoutAlgorithm::Radial, &nodes, &edges);

        let pos: HashMap<String, (f64, f64)> =
            result.into_iter().map(|p| (p.node_id, (p.x, p.y))).collect();

        let r_mid = (pos["mid"].0.powi(2) + pos["mid"].1.powi(2)).sqrt();
        let r_leaf = (pos["leaf"].0.powi(2) + pos["leaf"].1.powi(2)).sqrt();

        assert!(r_leaf > r_mid, "leaf should be farther from center than mid");
    }

    #[test]
    fn compute_layout_empty() {
        let result = compute_layout(&LayoutAlgorithm::Tree, &[], &[]);
        assert!(result.is_empty());
    }
}
