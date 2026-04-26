use axum::http::StatusCode;
use hive_api::{build_router, chat, AppState, ChatRuntimeConfig, ChatService, SchedulerService};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Boot a test server with a real knowledge-graph DB in a temp directory.
async fn boot_kg_server() -> (String, CancellationToken, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let mut config = HiveMindConfig::default();
    config.api.bind = addr.to_string();

    let audit = AuditLogger::new(tempdir.path().join("audit.log")).expect("audit");
    let event_bus = EventBus::new(32);
    let shutdown = CancellationToken::new();

    let model_router = chat::build_model_router_from_config(&config, None, None)
        .expect("model router from config");
    let chat = Arc::new(ChatService::with_model_router(
        audit.clone(),
        event_bus.clone(),
        ChatRuntimeConfig::default(),
        tempdir.path().to_path_buf(),
        tempdir.path().join("knowledge.db"),
        config.security.prompt_injection.clone(),
        hive_contracts::config::CommandPolicyConfig::default(),
        tempdir.path().join("risk-ledger.db"),
        model_router,
        hive_api::canvas_ws::CanvasSessionRegistry::new(),
        hive_contracts::ContextCompactionConfig::default(),
        "127.0.0.1:0".to_string(),
        Default::default(),
        None, // mcp
        None, // mcp_catalog
        None, // connector_registry
        None, // connector_audit_log
        None, // connector_service
        Arc::new(
            SchedulerService::in_memory(
                event_bus.clone(),
                hive_scheduler::SchedulerConfig::default(),
            )
            .expect("test scheduler"),
        ),
        Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
        Arc::new(hive_contracts::DetectedShells::default()),
        hive_contracts::ToolLimitsConfig::default(),
        hive_contracts::CodeActConfig::default(),
        None, // plugin_host
        None, // plugin_registry
    ));

    let kg_path = tempdir.path().join("kg-test.db");
    let mut state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);
    state.knowledge_graph_path = Arc::new(kg_path);

    let router = build_router(state);
    let server_shutdown = shutdown.clone();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_shutdown.cancelled().await })
            .await
            .expect("serve");
    });

    (format!("http://{addr}"), shutdown, tempdir)
}

/// POST helper returning the response.
async fn post_json(client: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    client.post(url).json(&body).send().await.expect("POST request")
}

/// Helper: create a node and return its id.
async fn create_node(
    client: &reqwest::Client,
    base: &str,
    node_type: &str,
    name: &str,
    content: Option<&str>,
) -> i64 {
    let mut body = json!({ "node_type": node_type, "name": name });
    if let Some(c) = content {
        body["content"] = json!(c);
    }
    let resp = post_json(client, &format!("{base}/api/v1/knowledge/nodes"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.expect("json");
    v["id"].as_i64().expect("id field")
}

/// Helper: create a node with a specific data_class and return its id.
async fn create_node_with_class(
    client: &reqwest::Client,
    base: &str,
    node_type: &str,
    name: &str,
    content: Option<&str>,
    data_class: &str, // must be SCREAMING_SNAKE_CASE: "PUBLIC", "INTERNAL", etc.
) -> i64 {
    let mut body = json!({ "node_type": node_type, "name": name, "data_class": data_class });
    if let Some(c) = content {
        body["content"] = json!(c);
    }
    let resp = post_json(client, &format!("{base}/api/v1/knowledge/nodes"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.expect("json");
    v["id"].as_i64().expect("id field")
}

/// Helper: create an edge and return its id.
async fn create_edge(
    client: &reqwest::Client,
    base: &str,
    source_id: i64,
    target_id: i64,
    edge_type: &str,
    weight: Option<f64>,
) -> i64 {
    let mut body = json!({
        "source_id": source_id,
        "target_id": target_id,
        "edge_type": edge_type,
    });
    if let Some(w) = weight {
        body["weight"] = json!(w);
    }
    let resp = post_json(client, &format!("{base}/api/v1/knowledge/edges"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.json::<Value>().await.unwrap()["id"].as_i64().unwrap()
}

fn authed_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer test-token".parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_and_get_node() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "function", "my_func", Some("does stuff")).await;

    let resp =
        client.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("GET node");
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["id"], id);
    assert_eq!(body["node_type"], "function");
    assert_eq!(body["name"], "my_func");
    assert_eq!(body["content"], "does stuff");

    shutdown.cancel();
}

#[tokio::test]
async fn test_list_nodes() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    create_node(&client, &base, "module", "mod_a", None).await;
    create_node(&client, &base, "module", "mod_b", None).await;
    create_node(&client, &base, "class", "ClassC", None).await;

    // List all nodes
    let resp = client.get(format!("{base}/api/v1/knowledge/nodes")).send().await.expect("list");
    assert_eq!(resp.status(), StatusCode::OK);
    let nodes: Vec<Value> = resp.json().await.expect("json");
    assert!(nodes.len() >= 3, "expected at least 3 nodes, got {}", nodes.len());

    // List filtered by node_type
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes?node_type=module"))
        .send()
        .await
        .expect("list filtered");
    assert_eq!(resp.status(), StatusCode::OK);
    let modules: Vec<Value> = resp.json().await.expect("json");
    assert_eq!(modules.len(), 2, "expected 2 modules, got {}", modules.len());

    shutdown.cancel();
}

#[tokio::test]
async fn test_delete_node() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "function", "to_delete", None).await;

    // Delete
    let resp =
        client.delete(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("DELETE");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET should now 404
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Deleting again should also 404
    let resp = client
        .delete(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("DELETE again");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    shutdown.cancel();
}

#[tokio::test]
async fn test_create_and_get_edge() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let src = create_node(&client, &base, "module", "src_mod", None).await;
    let tgt = create_node(&client, &base, "function", "tgt_fn", None).await;

    // Create edge
    let resp = post_json(
        &client,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({
            "source_id": src,
            "target_id": tgt,
            "edge_type": "contains",
            "weight": 1.5
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let edge_body: Value = resp.json().await.expect("json");
    let edge_id = edge_body["id"].as_i64().expect("edge id");
    assert!(edge_id > 0);

    // GET edges for source node
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{src}/edges"))
        .send()
        .await
        .expect("GET edges");
    assert_eq!(resp.status(), StatusCode::OK);
    let edges: Vec<Value> = resp.json().await.expect("json");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["source_id"], src);
    assert_eq!(edges[0]["target_id"], tgt);
    assert_eq!(edges[0]["edge_type"], "contains");

    shutdown.cancel();
}

#[tokio::test]
async fn test_delete_edge() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let n1 = create_node(&client, &base, "a", "n1", None).await;
    let n2 = create_node(&client, &base, "b", "n2", None).await;

    let resp = post_json(
        &client,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({ "source_id": n1, "target_id": n2, "edge_type": "refs" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let edge_id = resp.json::<Value>().await.unwrap()["id"].as_i64().unwrap();

    // Delete edge
    let resp = client
        .delete(format!("{base}/api/v1/knowledge/edges/{edge_id}"))
        .send()
        .await
        .expect("DELETE edge");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Edges for node should now be empty
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{n1}/edges"))
        .send()
        .await
        .expect("GET edges after delete");
    assert_eq!(resp.status(), StatusCode::OK);
    let edges: Vec<Value> = resp.json().await.expect("json");
    assert!(edges.is_empty(), "edges should be empty after delete");

    // Delete again → 404
    let resp = client
        .delete(format!("{base}/api/v1/knowledge/edges/{edge_id}"))
        .send()
        .await
        .expect("DELETE again");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    shutdown.cancel();
}

#[tokio::test]
async fn test_search_nodes() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    create_node(
        &client,
        &base,
        "function",
        "parse_json",
        Some("Parses a JSON string into a value"),
    )
    .await;
    create_node(
        &client,
        &base,
        "function",
        "format_output",
        Some("Formats the output for display"),
    )
    .await;
    create_node(&client, &base, "module", "network", Some("Handles HTTP network requests")).await;

    // Search for "JSON"
    let resp =
        client.get(format!("{base}/api/v1/knowledge/search?q=JSON")).send().await.expect("search");
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.expect("json");
    assert!(!results.is_empty(), "search for 'JSON' should return at least one result");
    let names: Vec<&str> = results.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(
        names.contains(&"parse_json"),
        "search results should contain parse_json, got: {names:?}"
    );

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_stats() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let n1 = create_node(&client, &base, "module", "stats_mod", None).await;
    let n2 = create_node(&client, &base, "function", "stats_fn", None).await;
    post_json(
        &client,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({ "source_id": n1, "target_id": n2, "edge_type": "contains" }),
    )
    .await;

    let resp = client.get(format!("{base}/api/v1/knowledge/stats")).send().await.expect("stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let stats: Value = resp.json().await.expect("json");
    assert!(stats["node_count"].as_i64().unwrap() >= 2, "expected at least 2 nodes");
    assert!(stats["edge_count"].as_i64().unwrap() >= 1, "expected at least 1 edge");
    assert!(
        stats["nodes_by_type"].as_array().unwrap().len() >= 2,
        "expected at least 2 node types"
    );
    assert!(
        !stats["edges_by_type"].as_array().unwrap().is_empty(),
        "expected at least 1 edge type"
    );

    shutdown.cancel();
}

// ---------------------------------------------------------------------------
// New endpoint tests: neighbors, update, vector search, and complex scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_neighbors_returns_connected_nodes() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let hub = create_node(&client, &base, "module", "core_module", Some("central hub")).await;
    let n1 = create_node(&client, &base, "function", "func_a", None).await;
    let n2 = create_node(&client, &base, "function", "func_b", None).await;
    let n3 = create_node(&client, &base, "class", "MyClass", None).await;

    create_edge(&client, &base, hub, n1, "contains", None).await;
    create_edge(&client, &base, hub, n2, "contains", None).await;
    create_edge(&client, &base, n3, hub, "depends_on", None).await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{hub}/neighbors"))
        .send()
        .await
        .expect("GET neighbors");
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["id"], hub);
    assert_eq!(body["name"], "core_module");

    let edges = body["edges"].as_array().expect("edges array");
    assert_eq!(edges.len(), 3, "hub should have 3 edges");

    let neighbors = body["neighbors"].as_array().expect("neighbors array");
    assert_eq!(neighbors.len(), 3, "hub should have 3 neighbors");

    let neighbor_ids: Vec<i64> = neighbors.iter().map(|n| n["id"].as_i64().unwrap()).collect();
    assert!(neighbor_ids.contains(&n1));
    assert!(neighbor_ids.contains(&n2));
    assert!(neighbor_ids.contains(&n3));

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_neighbors_with_limit() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let hub = create_node(&client, &base, "module", "big_module", None).await;
    for i in 0..5 {
        let child = create_node(&client, &base, "function", &format!("fn_{i}"), None).await;
        create_edge(&client, &base, hub, child, "contains", None).await;
    }

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{hub}/neighbors?limit=2"))
        .send()
        .await
        .expect("GET neighbors limited");
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.expect("json");
    let neighbors = body["neighbors"].as_array().expect("neighbors");
    assert!(neighbors.len() <= 2, "should respect limit, got {}", neighbors.len());

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_neighbors_isolated_node() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let isolated = create_node(&client, &base, "constant", "PI", Some("3.14159")).await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{isolated}/neighbors"))
        .send()
        .await
        .expect("GET neighbors isolated");
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["id"], isolated);
    assert!(body["edges"].as_array().unwrap().is_empty());
    assert!(body["neighbors"].as_array().unwrap().is_empty());

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_neighbors_nonexistent_node() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/999999/neighbors"))
        .send()
        .await
        .expect("GET neighbors missing");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_neighbors_bidirectional_edges() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let a = create_node(&client, &base, "module", "mod_x", None).await;
    let b = create_node(&client, &base, "module", "mod_y", None).await;

    create_edge(&client, &base, a, b, "imports", None).await;
    create_edge(&client, &base, b, a, "imports", None).await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{a}/neighbors"))
        .send()
        .await
        .expect("GET neighbors a");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    let neighbors = body["neighbors"].as_array().unwrap();
    assert_eq!(neighbors.len(), 1, "b should appear once despite 2 edges");
    assert_eq!(neighbors[0]["id"], b);

    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "should have both edges");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_node_name() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "function", "old_name", Some("original content")).await;

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({ "name": "new_name" }))
        .send()
        .await
        .expect("PUT update name");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET updated");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "new_name");
    assert_eq!(body["content"], "original content");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_node_content() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "function", "stable_name", Some("v1 content")).await;

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({ "content": "v2 content with more detail" }))
        .send()
        .await
        .expect("PUT update content");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET updated");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "stable_name");
    assert_eq!(body["content"], "v2 content with more detail");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_node_data_class() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id =
        create_node_with_class(&client, &base, "function", "classified_fn", None, "INTERNAL").await;

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({ "data_class": "PUBLIC" }))
        .send()
        .await
        .expect("PUT update data_class");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET updated");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["data_class"], "public");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_node_multiple_fields() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "class", "OldClass", Some("old doc")).await;

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({
            "name": "RenamedClass",
            "content": "updated documentation",
            "data_class": "PUBLIC"
        }))
        .send()
        .await
        .expect("PUT update all");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET after multi-update");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "RenamedClass");
    assert_eq!(body["content"], "updated documentation");
    assert_eq!(body["data_class"], "public");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_node_empty_body_is_noop() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "function", "no_change", Some("same")).await;

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({}))
        .send()
        .await
        .expect("PUT empty");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET unchanged");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "no_change");
    assert_eq!(body["content"], "same");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_nonexistent_node_returns_404() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/999999"))
        .json(&json!({ "name": "ghost" }))
        .send()
        .await
        .expect("PUT missing");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    shutdown.cancel();
}

#[tokio::test]
async fn test_vector_search_without_runtime_returns_503() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let resp = client
        .get(format!("{base}/api/v1/knowledge/search/vector?q=hello"))
        .send()
        .await
        .expect("vector search");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "vector search should 503 without runtime_manager"
    );

    shutdown.cancel();
}

#[tokio::test]
async fn test_create_node_without_content() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "variable", "counter", None).await;

    let resp =
        client.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("GET node");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "counter");
    assert!(body["content"].is_null(), "content should be null when not provided");

    shutdown.cancel();
}

#[tokio::test]
async fn test_create_node_with_data_class() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node_with_class(
        &client,
        &base,
        "function",
        "public_api",
        Some("public endpoint"),
        "PUBLIC",
    )
    .await;

    let resp =
        client.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("GET node");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["data_class"], "public");

    shutdown.cancel();
}

#[tokio::test]
async fn test_list_nodes_with_limit() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    for i in 0..10 {
        create_node(&client, &base, "function", &format!("fn_{i}"), None).await;
    }

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes?limit=3"))
        .send()
        .await
        .expect("list limited");
    assert_eq!(resp.status(), StatusCode::OK);
    let nodes: Vec<Value> = resp.json().await.expect("json");
    assert_eq!(nodes.len(), 3, "should respect limit");

    shutdown.cancel();
}

#[tokio::test]
async fn test_get_node_includes_edges() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let parent = create_node(&client, &base, "module", "parent_mod", None).await;
    let child = create_node(&client, &base, "function", "child_fn", None).await;
    create_edge(&client, &base, parent, child, "contains", Some(2.0)).await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{parent}"))
        .send()
        .await
        .expect("GET node with edges");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    let edges = body["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["edge_type"], "contains");
    assert_eq!(edges[0]["weight"], 2.0);

    shutdown.cancel();
}

#[tokio::test]
async fn test_delete_node_cascades_edges() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let a = create_node(&client, &base, "module", "to_remove", None).await;
    let b = create_node(&client, &base, "function", "stays", None).await;
    create_edge(&client, &base, a, b, "contains", None).await;

    let resp =
        client.delete(format!("{base}/api/v1/knowledge/nodes/{a}")).send().await.expect("DELETE");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{b}/edges"))
        .send()
        .await
        .expect("GET edges");
    let edges: Vec<Value> = resp.json().await.expect("json");
    assert!(edges.is_empty(), "edges referencing deleted node should be gone");

    shutdown.cancel();
}

#[tokio::test]
async fn test_edge_default_weight() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let a = create_node(&client, &base, "a", "node_a", None).await;
    let b = create_node(&client, &base, "b", "node_b", None).await;

    let resp = post_json(
        &client,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({ "source_id": a, "target_id": b, "edge_type": "links" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{a}/edges"))
        .send()
        .await
        .expect("GET edges");
    let edges: Vec<Value> = resp.json().await.expect("json");
    assert_eq!(edges[0]["weight"], 1.0, "default weight should be 1.0");

    shutdown.cancel();
}

#[tokio::test]
async fn test_search_no_results() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    create_node(&client, &base, "function", "hello_world", Some("prints greeting")).await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/search?q=xyznonexistent123"))
        .send()
        .await
        .expect("search");
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.expect("json");
    assert!(results.is_empty(), "no results expected for nonsense query");

    shutdown.cancel();
}

#[tokio::test]
async fn test_search_with_limit() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    for i in 0..8 {
        create_node(
            &client,
            &base,
            "function",
            &format!("parser_{i}"),
            Some(&format!("parses format {i}")),
        )
        .await;
    }

    let resp = client
        .get(format!("{base}/api/v1/knowledge/search?q=parser&limit=3"))
        .send()
        .await
        .expect("search limited");
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.expect("json");
    assert!(results.len() <= 3, "should respect limit, got {}", results.len());

    shutdown.cancel();
}

#[tokio::test]
async fn test_stats_empty_graph() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let resp = client.get(format!("{base}/api/v1/knowledge/stats")).send().await.expect("stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let stats: Value = resp.json().await.expect("json");
    assert_eq!(stats["node_count"], 0);
    assert_eq!(stats["edge_count"], 0);
    assert!(stats["nodes_by_type"].as_array().unwrap().is_empty());
    assert!(stats["edges_by_type"].as_array().unwrap().is_empty());

    shutdown.cancel();
}

#[tokio::test]
async fn test_complex_graph_traversal() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    // Build: mod_a -> fn_1 -> class_x <- fn_2 <- mod_b
    let mod_a = create_node(&client, &base, "module", "mod_a", None).await;
    let fn_1 = create_node(&client, &base, "function", "fn_1", None).await;
    let class_x = create_node(&client, &base, "class", "ClassX", None).await;
    let fn_2 = create_node(&client, &base, "function", "fn_2", None).await;
    let mod_b = create_node(&client, &base, "module", "mod_b", None).await;

    create_edge(&client, &base, mod_a, fn_1, "contains", None).await;
    create_edge(&client, &base, fn_1, class_x, "uses", None).await;
    create_edge(&client, &base, fn_2, class_x, "uses", None).await;
    create_edge(&client, &base, mod_b, fn_2, "contains", None).await;

    // class_x neighbors: fn_1 and fn_2 (1 hop), NOT mod_a/mod_b (2 hops)
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{class_x}/neighbors"))
        .send()
        .await
        .expect("GET neighbors");
    let body: Value = resp.json().await.expect("json");
    let neighbor_ids: Vec<i64> =
        body["neighbors"].as_array().unwrap().iter().map(|n| n["id"].as_i64().unwrap()).collect();
    assert!(neighbor_ids.contains(&fn_1));
    assert!(neighbor_ids.contains(&fn_2));
    assert!(!neighbor_ids.contains(&mod_a), "mod_a is 2 hops away");
    assert!(!neighbor_ids.contains(&mod_b), "mod_b is 2 hops away");

    // fn_1 neighbors: mod_a and class_x
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{fn_1}/neighbors"))
        .send()
        .await
        .expect("GET fn_1 neighbors");
    let body: Value = resp.json().await.expect("json");
    let neighbor_ids: Vec<i64> =
        body["neighbors"].as_array().unwrap().iter().map(|n| n["id"].as_i64().unwrap()).collect();
    assert!(neighbor_ids.contains(&mod_a));
    assert!(neighbor_ids.contains(&class_x));

    shutdown.cancel();
}

#[tokio::test]
async fn test_stats_reflect_types() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let m1 = create_node(&client, &base, "module", "m1", None).await;
    let m2 = create_node(&client, &base, "module", "m2", None).await;
    let f1 = create_node(&client, &base, "function", "f1", None).await;
    create_edge(&client, &base, m1, f1, "contains", None).await;
    create_edge(&client, &base, m2, f1, "imports", None).await;

    let resp = client.get(format!("{base}/api/v1/knowledge/stats")).send().await.expect("stats");
    let stats: Value = resp.json().await.expect("json");
    assert_eq!(stats["node_count"], 3);
    assert_eq!(stats["edge_count"], 2);

    let node_types = stats["nodes_by_type"].as_array().unwrap();
    let module_count = node_types.iter().find(|t| t["name"] == "module").expect("module type")
        ["count"]
        .as_i64()
        .unwrap();
    assert_eq!(module_count, 2);

    let edge_types = stats["edges_by_type"].as_array().unwrap();
    assert_eq!(edge_types.len(), 2, "should have 2 edge types");

    shutdown.cancel();
}

#[tokio::test]
async fn test_update_then_search_reflects_changes() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id =
        create_node(&client, &base, "function", "transform_data", Some("transforms input data"))
            .await;

    // Update content to unique keyword
    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({ "content": "serializes protobuf messages" }))
        .send()
        .await
        .expect("PUT update");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Search for the new content
    let resp = client
        .get(format!("{base}/api/v1/knowledge/search?q=protobuf"))
        .send()
        .await
        .expect("search");
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.expect("json");
    assert!(!results.is_empty(), "search should find node by updated content");
    let found = results.iter().any(|r| r["id"] == id);
    assert!(found, "updated node should appear in search results");

    shutdown.cancel();
}

#[tokio::test]
async fn test_neighbors_after_edge_deletion() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let hub = create_node(&client, &base, "module", "hub", None).await;
    let child1 = create_node(&client, &base, "function", "child1", None).await;
    let child2 = create_node(&client, &base, "function", "child2", None).await;

    let edge1 = create_edge(&client, &base, hub, child1, "contains", None).await;
    create_edge(&client, &base, hub, child2, "contains", None).await;

    // Verify 2 neighbors
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{hub}/neighbors"))
        .send()
        .await
        .expect("GET neighbors before");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["neighbors"].as_array().unwrap().len(), 2);

    // Delete one edge
    let resp = client
        .delete(format!("{base}/api/v1/knowledge/edges/{edge1}"))
        .send()
        .await
        .expect("DELETE edge");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Now only 1 neighbor
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{hub}/neighbors"))
        .send()
        .await
        .expect("GET neighbors after");
    let body: Value = resp.json().await.expect("json");
    let neighbors = body["neighbors"].as_array().unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0]["id"], child2);

    shutdown.cancel();
}

#[tokio::test]
async fn test_list_nodes_filter_by_data_class() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    create_node_with_class(&client, &base, "function", "pub_fn", None, "PUBLIC").await;
    create_node_with_class(&client, &base, "function", "int_fn", None, "INTERNAL").await;
    create_node_with_class(&client, &base, "function", "res_fn", None, "RESTRICTED").await;

    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes?data_class=PUBLIC"))
        .send()
        .await
        .expect("list public");
    assert_eq!(resp.status(), StatusCode::OK);
    let nodes: Vec<Value> = resp.json().await.expect("json");
    assert!(
        nodes.iter().all(|n| n["data_class"] == "public"),
        "all returned nodes should be Public"
    );
    assert!(nodes.iter().any(|n| n["name"] == "pub_fn"), "pub_fn should be present");

    shutdown.cancel();
}

#[tokio::test]
async fn test_node_timestamps_are_present() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    let id = create_node(&client, &base, "module", "timestamped", Some("test timestamps")).await;

    let resp =
        client.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("GET node");
    let body: Value = resp.json().await.expect("json");

    assert!(body["created_at"].is_string(), "created_at should be present");
    assert!(body["updated_at"].is_string(), "updated_at should be present");

    shutdown.cancel();
}

#[tokio::test]
async fn test_crud_lifecycle() {
    let (base, shutdown, _dir) = boot_kg_server().await;
    let client = authed_client();

    // Create
    let id = create_node(&client, &base, "function", "lifecycle_fn", Some("v1")).await;

    // Read
    let resp = client.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("GET");
    assert_eq!(resp.status(), StatusCode::OK);

    // Update
    let resp = client
        .put(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .json(&json!({ "name": "lifecycle_fn_v2", "content": "v2" }))
        .send()
        .await
        .expect("PUT");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify update
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET after update");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["name"], "lifecycle_fn_v2");
    assert_eq!(body["content"], "v2");

    // Delete
    let resp =
        client.delete(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.expect("DELETE");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify deletion
    let resp = client
        .get(format!("{base}/api/v1/knowledge/nodes/{id}"))
        .send()
        .await
        .expect("GET after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Stats should reflect removal
    let resp = client.get(format!("{base}/api/v1/knowledge/stats")).send().await.expect("stats");
    let stats: Value = resp.json().await.expect("json");
    assert_eq!(stats["node_count"], 0);

    shutdown.cancel();
}
