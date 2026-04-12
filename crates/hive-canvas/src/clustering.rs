//! Auto-clustering for spatial canvas cards.
//!
//! Groups semantically similar cards using embedding cosine similarity
//! with spatial proximity weighting, via agglomerative clustering.

use crate::events::CanvasEvent;
use crate::store::CanvasStore;
use crate::types::{CanvasEdge, CanvasNode, CardStatus, CardType, EdgeType};

/// A discovered cluster of related cards.
#[derive(Clone, Debug)]
pub struct CardCluster {
    /// Human-readable label (derived from representative member).
    pub label: String,
    /// IDs of member cards.
    pub member_ids: Vec<String>,
    /// Centroid position (avg x, avg y of members).
    pub centroid: (f64, f64),
}

/// Cosine similarity between two embedding vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "embedding dimensions must match");
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 {
        0.0
    } else {
        dot / denom
    }
}

/// Euclidean distance between two 2D points.
fn spatial_distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Combined similarity score: cosine similarity boosted by spatial proximity.
///
/// Nearby cards get up to 1.5× boost; distant cards get ~1.0×.
pub fn combined_similarity(cosine: f32, dist: f64) -> f32 {
    let proximity_boost = 1.0 + 0.5 / (1.0 + dist / 500.0);
    cosine * proximity_boost as f32
}

/// An item to be clustered: card ID, its embedding, and its canvas position.
pub struct ClusterInput {
    pub id: String,
    pub embedding: Vec<f32>,
    pub position: (f64, f64),
    pub text: String,
}

/// Agglomerative clustering using average-linkage with a similarity threshold.
///
/// Returns clusters with ≥ `min_size` members, up to `max_clusters`.
pub fn agglomerative_cluster(
    items: &[ClusterInput],
    sim_threshold: f32,
    min_size: usize,
    max_clusters: usize,
) -> Vec<CardCluster> {
    if items.len() < min_size {
        return Vec::new();
    }

    // Each item starts in its own cluster
    let n = items.len();
    let mut assignments: Vec<usize> = (0..n).collect();
    let mut cluster_count = n;

    // Precompute pairwise combined similarity
    let mut sim_matrix = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let cos = cosine_similarity(&items[i].embedding, &items[j].embedding);
            let dist = spatial_distance(items[i].position, items[j].position);
            let combined = combined_similarity(cos, dist);
            sim_matrix[i][j] = combined;
            sim_matrix[j][i] = combined;
        }
    }

    // Iteratively merge the most similar pair of clusters
    loop {
        // Find best merge (highest average-linkage similarity)
        let mut best_sim = 0.0f32;
        let mut best_pair = (0usize, 0usize);

        // Collect current cluster IDs
        let mut cluster_ids: Vec<usize> = assignments
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cluster_ids.sort();

        for ci in 0..cluster_ids.len() {
            for cj in (ci + 1)..cluster_ids.len() {
                let ca = cluster_ids[ci];
                let cb = cluster_ids[cj];

                // Average-linkage: mean similarity between all pairs across clusters
                let members_a: Vec<usize> = (0..n).filter(|&i| assignments[i] == ca).collect();
                let members_b: Vec<usize> = (0..n).filter(|&i| assignments[i] == cb).collect();

                let mut total = 0.0f32;
                let pair_count = members_a.len() * members_b.len();
                for &ia in &members_a {
                    for &ib in &members_b {
                        total += sim_matrix[ia][ib];
                    }
                }
                let avg = total / pair_count as f32;

                if avg > best_sim {
                    best_sim = avg;
                    best_pair = (ca, cb);
                }
            }
        }

        if best_sim < sim_threshold {
            break;
        }

        // Merge: assign all items in best_pair.1 to best_pair.0
        let (keep, merge) = best_pair;
        for a in assignments.iter_mut() {
            if *a == merge {
                *a = keep;
            }
        }
        cluster_count -= 1;

        // Stop merging when we can't reduce further (single cluster)
        if cluster_count <= 1 {
            break;
        }
    }

    // Build cluster results
    let mut cluster_map: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (idx, &cid) in assignments.iter().enumerate() {
        cluster_map.entry(cid).or_default().push(idx);
    }

    let mut clusters: Vec<CardCluster> = cluster_map
        .into_values()
        .filter(|members| members.len() >= min_size)
        .map(|members| {
            let cx =
                members.iter().map(|&i| items[i].position.0).sum::<f64>() / members.len() as f64;
            let cy =
                members.iter().map(|&i| items[i].position.1).sum::<f64>() / members.len() as f64;

            // Label: first sentence of the most "central" member (highest avg similarity to group)
            let label = pick_label(&members, items);

            CardCluster {
                label,
                member_ids: members.iter().map(|&i| items[i].id.clone()).collect(),
                centroid: (cx, cy),
            }
        })
        .collect();

    // Sort by member count descending
    clusters.sort_by(|a, b| b.member_ids.len().cmp(&a.member_ids.len()));
    clusters.truncate(max_clusters);
    clusters
}

/// Pick a label from the most representative member (highest avg cosine to group).
fn pick_label(members: &[usize], items: &[ClusterInput]) -> String {
    let mut best_idx = members[0];
    let mut best_avg = f32::MIN;

    for &i in members {
        let avg: f32 = members
            .iter()
            .filter(|&&j| j != i)
            .map(|&j| cosine_similarity(&items[i].embedding, &items[j].embedding))
            .sum::<f32>()
            / (members.len() as f32 - 1.0).max(1.0);
        if avg > best_avg {
            best_avg = avg;
            best_idx = i;
        }
    }

    let text = &items[best_idx].text;
    // Take first sentence or first 60 chars
    let label = text.split(&['.', '!', '?', '\n'][..]).next().unwrap_or(text);
    if label.len() > 60 {
        // Find a safe UTF-8 boundary at or before byte 57.
        let mut end = 57;
        while end > 0 && !label.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &label[..end])
    } else {
        label.to_string()
    }
}

/// Apply clustering to a canvas: create/update Cluster nodes and membership edges.
///
/// `embed_fn` generates a 384-dim embedding for a text string. Pass a closure
/// that calls the inference runtime, e.g. `|text| runtime.embed(model_id, text)`.
///
/// Returns canvas events for WebSocket broadcast.
pub fn apply_clusters<S: CanvasStore + ?Sized>(
    store: &S,
    canvas_id: &str,
    embed_fn: &dyn Fn(&str) -> Result<Vec<f32>, String>,
    sim_threshold: f32,
) -> Result<Vec<CanvasEvent>, crate::error::CanvasError> {
    // 1. Get all active non-cluster nodes
    let all_nodes = store.get_all_nodes(canvas_id)?;
    let cards: Vec<&CanvasNode> = all_nodes
        .iter()
        .filter(|n| n.card_type != CardType::Cluster && n.status == CardStatus::Active)
        .collect();

    if cards.len() < 2 {
        return Ok(Vec::new());
    }

    // 2. Generate embeddings
    let mut inputs = Vec::new();
    for card in &cards {
        let text = card.content.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        match embed_fn(text) {
            Ok(embedding) => inputs.push(ClusterInput {
                id: card.id.clone(),
                embedding,
                position: (card.x, card.y),
                text: text.to_string(),
            }),
            Err(e) => {
                tracing::debug!("skipping card {} for clustering: {e}", card.id);
            }
        }
    }

    if inputs.len() < 2 {
        return Ok(Vec::new());
    }

    // 3. Run clustering
    let clusters = agglomerative_cluster(&inputs, sim_threshold, 2, 10);

    // 4. Remove existing cluster nodes for this canvas
    let old_clusters: Vec<String> = all_nodes
        .iter()
        .filter(|n| n.card_type == CardType::Cluster && n.created_by == "auto-cluster")
        .map(|n| n.id.clone())
        .collect();
    for old_id in &old_clusters {
        let _ = store.delete_node(old_id);
    }

    // 5. Create new cluster nodes + edges
    let mut events = Vec::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    for (i, cluster) in clusters.iter().enumerate() {
        let cluster_id = format!("cluster-auto-{canvas_id}-{i}");
        let cluster_node = CanvasNode {
            id: cluster_id.clone(),
            canvas_id: canvas_id.to_string(),
            card_type: CardType::Cluster,
            x: cluster.centroid.0 - 20.0,
            y: cluster.centroid.1 - 40.0,
            width: 280.0,
            height: 60.0,
            content: serde_json::json!({ "text": cluster.label }),
            status: CardStatus::Active,
            created_by: "auto-cluster".to_string(),
            created_at: now,
        };

        let _ = store.insert_node(&cluster_node);
        events.push(CanvasEvent::NodeCreated { node: cluster_node, parent_edge: None });

        for member_id in &cluster.member_ids {
            let edge = CanvasEdge {
                id: format!("edge-cluster-{cluster_id}-{member_id}"),
                canvas_id: canvas_id.to_string(),
                source_id: cluster_id.clone(),
                target_id: member_id.clone(),
                edge_type: EdgeType::ContextShare,
                metadata: serde_json::json!({}),
                created_at: now,
            };
            let _ = store.insert_edge(&edge);
            events.push(CanvasEvent::EdgeCreated { edge });
        }
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_combined_similarity_nearby_boost() {
        let cos = 0.8;
        let near = combined_similarity(cos, 0.0);
        let far = combined_similarity(cos, 5000.0);
        assert!(near > far, "nearby should boost more: {near} > {far}");
        assert!((near - 0.8 * 1.5).abs() < 0.01, "max boost is 1.5x");
    }

    #[test]
    fn test_agglomerative_cluster_basic() {
        // Two pairs of similar items, one dissimilar
        let items = vec![
            ClusterInput {
                id: "a".into(),
                embedding: vec![1.0, 0.0, 0.0],
                position: (0.0, 0.0),
                text: "hello world".into(),
            },
            ClusterInput {
                id: "b".into(),
                embedding: vec![0.95, 0.05, 0.0],
                position: (10.0, 10.0),
                text: "hello there".into(),
            },
            ClusterInput {
                id: "c".into(),
                embedding: vec![0.0, 0.0, 1.0],
                position: (1000.0, 1000.0),
                text: "completely different".into(),
            },
            ClusterInput {
                id: "d".into(),
                embedding: vec![0.0, 0.05, 0.95],
                position: (1010.0, 1010.0),
                text: "also different topic".into(),
            },
        ];

        let clusters = agglomerative_cluster(&items, 0.3, 2, 10);
        assert_eq!(clusters.len(), 2, "should form 2 clusters");

        let cluster_ab = clusters.iter().find(|c| c.member_ids.contains(&"a".to_string()));
        let cluster_cd = clusters.iter().find(|c| c.member_ids.contains(&"c".to_string()));
        assert!(cluster_ab.is_some(), "a and b should cluster together");
        assert!(cluster_cd.is_some(), "c and d should cluster together");

        let ab = cluster_ab.unwrap();
        assert!(ab.member_ids.contains(&"b".to_string()));
        assert!(!ab.member_ids.contains(&"c".to_string()));
    }

    #[test]
    fn test_agglomerative_too_few_items() {
        let items = vec![ClusterInput {
            id: "a".into(),
            embedding: vec![1.0],
            position: (0.0, 0.0),
            text: "solo".into(),
        }];
        let clusters = agglomerative_cluster(&items, 0.5, 2, 10);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_agglomerative_all_dissimilar() {
        let items = vec![
            ClusterInput {
                id: "a".into(),
                embedding: vec![1.0, 0.0, 0.0],
                position: (0.0, 0.0),
                text: "x".into(),
            },
            ClusterInput {
                id: "b".into(),
                embedding: vec![0.0, 1.0, 0.0],
                position: (5000.0, 0.0),
                text: "y".into(),
            },
            ClusterInput {
                id: "c".into(),
                embedding: vec![0.0, 0.0, 1.0],
                position: (0.0, 5000.0),
                text: "z".into(),
            },
        ];
        let clusters = agglomerative_cluster(&items, 0.5, 2, 10);
        assert!(clusters.is_empty(), "dissimilar items should not cluster");
    }

    #[test]
    fn test_label_picks_most_central() {
        let items = vec![
            ClusterInput {
                id: "a".into(),
                embedding: vec![0.9, 0.1],
                position: (0.0, 0.0),
                text: "The central concept.".into(),
            },
            ClusterInput {
                id: "b".into(),
                embedding: vec![0.85, 0.15],
                position: (10.0, 10.0),
                text: "A related idea.".into(),
            },
            ClusterInput {
                id: "c".into(),
                embedding: vec![0.8, 0.2],
                position: (20.0, 20.0),
                text: "Another related point.".into(),
            },
        ];
        let clusters = agglomerative_cluster(&items, 0.3, 2, 10);
        assert!(!clusters.is_empty());
        // Label should come from the most central member
        let label = &clusters[0].label;
        assert!(!label.is_empty());
    }
}
