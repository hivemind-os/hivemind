/// Integration tests: connect to the mock MCP server using rmcp,
/// exercising the exact same code paths as hive-mcp.
///
/// Prerequisites: run `npm run compile` in tools/mock-mcp-server/ first.
use rmcp::ServiceExt;
use tokio::process::Command;

/// Find the repo root by walking up from CARGO_MANIFEST_DIR
fn repo_root() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // hive-mcp is at crates/hive-mcp, so repo root is ../../
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

fn mock_server_entry() -> std::path::PathBuf {
    repo_root().join("tools/mock-mcp-server/dist/index.js")
}

fn check_mock_server_built() {
    let entry = mock_server_entry();
    if !entry.exists() {
        panic!(
            "Mock MCP server not built. Run: cd tools/mock-mcp-server && npm run compile\n\
             Expected: {}",
            entry.display()
        );
    }
}

/// Spawns stderr reader task so child output isn't lost
fn spawn_stderr_reader(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[mock-server stderr] {}", line);
        }
    });
}

/// Run standard tool operations against a connected client.
async fn run_tool_tests(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
) -> anyhow::Result<()> {
    // List tools
    let tools = client.list_all_tools().await?;
    eprintln!("  ✅ Listed {} tools", tools.len());
    assert!(!tools.is_empty(), "Expected at least one tool");

    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        tool_names.contains(&"get_weather"),
        "Expected get_weather tool, got: {:?}",
        tool_names
    );

    // Call a tool
    let result = client
        .call_tool(rmcp::model::CallToolRequestParam {
            name: "get_weather".into(),
            arguments: Some(serde_json::json!({ "city": "Seattle" }).as_object().unwrap().clone()),
        })
        .await?;
    assert!(!result.content.is_empty(), "Expected non-empty tool result");
    eprintln!("  ✅ Tool call succeeded");

    // Call unknown tool
    let err_result = client
        .call_tool(rmcp::model::CallToolRequestParam {
            name: "nonexistent_tool".into(),
            arguments: None,
        })
        .await?;
    assert!(err_result.is_error.unwrap_or(false), "Expected isError=true for unknown tool");
    eprintln!("  ✅ Unknown tool correctly returned error");

    Ok(())
}

// ==========================================================================
// Test 1: Stdio transport (same code path as hive-mcp connect for stdio)
// ==========================================================================
#[tokio::test]
async fn test_mock_mcp_stdio() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    check_mock_server_built();

    let mut child = Command::new("node")
        .arg(mock_server_entry())
        .arg("--dashboard-port")
        .arg("0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn node");

    let child_stdout = child.stdout.take().unwrap();
    let child_stdin = child.stdin.take().unwrap();
    spawn_stderr_reader(child.stderr.take().unwrap());

    eprintln!("--- Stdio transport ---");
    let client = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ().serve((child_stdout, child_stdin)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Handshake timed out"))?
    .map_err(|e| anyhow::anyhow!("Handshake failed: {e}"))?;

    eprintln!("  ✅ Handshake succeeded");
    run_tool_tests(&client).await?;
    client.cancel().await?;
    eprintln!("--- Stdio: ALL PASSED ---");
    Ok(())
}

// ==========================================================================
// Test 2: SSE transport (same code path as hive-mcp connect for SSE)
// ==========================================================================
#[tokio::test]
async fn test_mock_mcp_sse() -> anyhow::Result<()> {
    use rmcp::transport::SseTransport;

    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    check_mock_server_built();

    // Start mock server in HTTP mode on a random-ish port
    let port = 16200u16;
    let mut child = Command::new("node")
        .arg(mock_server_entry())
        .arg("--mode")
        .arg("http")
        .arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn node");

    spawn_stderr_reader(child.stderr.take().unwrap());

    // Wait for server to be ready
    let sse_url = format!("http://127.0.0.1:{port}/sse");
    wait_for_http_ready(&format!("http://127.0.0.1:{port}/api/tools"), 15).await?;

    eprintln!("--- SSE transport ---");
    let transport =
        tokio::time::timeout(std::time::Duration::from_secs(10), SseTransport::start(&sse_url))
            .await
            .map_err(|_| anyhow::anyhow!("SSE connect timed out"))?
            .map_err(|e| anyhow::anyhow!("SSE connect failed: {e}"))?;

    let client = tokio::time::timeout(std::time::Duration::from_secs(10), ().serve(transport))
        .await
        .map_err(|_| anyhow::anyhow!("SSE handshake timed out"))?
        .map_err(|e| anyhow::anyhow!("SSE handshake failed: {e}"))?;

    eprintln!("  ✅ SSE handshake succeeded");
    run_tool_tests(&client).await?;
    client.cancel().await?;
    child.kill().await.ok();
    eprintln!("--- SSE: ALL PASSED ---");
    Ok(())
}

// ==========================================================================
// Test 3: Streamable HTTP transport (same code path as hive-mcp)
// ==========================================================================
#[tokio::test]
async fn test_mock_mcp_streamable_http() -> anyhow::Result<()> {
    use hive_mcp::streamable_http::StreamableHttpTransport;

    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    check_mock_server_built();

    let port = 16201u16;
    let mut child = Command::new("node")
        .arg(mock_server_entry())
        .arg("--mode")
        .arg("http")
        .arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn node");

    spawn_stderr_reader(child.stderr.take().unwrap());

    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    wait_for_http_ready(&format!("http://127.0.0.1:{port}/api/tools"), 15).await?;

    eprintln!("--- Streamable HTTP transport ---");
    let transport = StreamableHttpTransport::new(&mcp_url)
        .map_err(|e| anyhow::anyhow!("Transport create failed: {e}"))?;

    let client = tokio::time::timeout(std::time::Duration::from_secs(10), ().serve(transport))
        .await
        .map_err(|_| anyhow::anyhow!("Streamable HTTP handshake timed out"))?
        .map_err(|e| anyhow::anyhow!("Streamable HTTP handshake failed: {e}"))?;

    eprintln!("  ✅ Streamable HTTP handshake succeeded");
    run_tool_tests(&client).await?;
    client.cancel().await?;
    child.kill().await.ok();
    eprintln!("--- Streamable HTTP: ALL PASSED ---");
    Ok(())
}

/// Poll an HTTP endpoint until it responds (or timeout).
async fn wait_for_http_ready(url: &str, max_seconds: u32) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    for i in 0..max_seconds * 2 {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => continue,
        }
    }
    anyhow::bail!("Server at {url} not ready after {max_seconds}s")
}

// ==========================================================================
// Test 4: Sandboxed stdio — proves the real sandbox allows the MCP server
// to start and serve tools when given the correct workspace path.
//
// This test caught the bug where SessionMcpManager::connect() was NOT
// passing workspace_path to the sandbox policy builder, causing the
// seatbelt profile to deny reads on the server's own files.
// ==========================================================================
#[tokio::test]
async fn test_sandboxed_stdio_with_workspace() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    check_mock_server_built();

    // Only run on macOS where sandbox-exec is available.
    if cfg!(not(target_os = "macos")) {
        eprintln!("--- Skipping sandboxed test (not macOS) ---");
        return Ok(());
    }

    let workspace = repo_root();
    let entry = mock_server_entry();

    // Build a sandbox policy just like hive-mcp does, WITH workspace.
    let mut builder = hive_sandbox::SandboxPolicy::builder().network(true); // dashboard needs network
    builder = builder.allow_read_write(&workspace);
    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }
    builder = builder.allow_read_write(std::env::temp_dir());
    // Allow PATH dirs under $HOME (nvm, pyenv, etc.)
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if dir.starts_with(&home) {
                    builder = builder.allow_read(&dir);
                }
            }
        }
        // HiveMind OS home
        let hivemind_home = home.join(".hivemind");
        if hivemind_home.exists() {
            builder = builder.allow_read(&hivemind_home);
        }
    }
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }
    let policy = builder.build();

    // Wrap through the real sandbox
    let full_cmd = format!("node {} --dashboard-port 0", entry.display());
    let sandboxed = hive_sandbox::sandbox_command(&full_cmd, &policy);

    match sandboxed {
        Ok(hive_sandbox::SandboxedCommand::Wrapped { .. }) => {
            // Good — sandbox available, continue with test
        }
        _ => {
            eprintln!("--- Skipping: sandbox-exec not available ---");
            return Ok(());
        }
    };

    // Re-create the sandboxed command so _temp_files lives long enough.
    // sandbox_command writes a temp profile file that must exist when sandbox-exec reads it.
    let sandboxed = hive_sandbox::sandbox_command(&full_cmd, &policy).unwrap();
    let (program, args, _temp_files) = match sandboxed {
        hive_sandbox::SandboxedCommand::Wrapped { program, args, _temp_files } => {
            (program, args, _temp_files)
        }
        _ => unreachable!(),
    };

    eprintln!("--- Sandboxed stdio (with workspace) ---");
    eprintln!("  Command: {} {}", program, args.join(" "));

    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(&workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn sandboxed node");

    let child_stdout = child.stdout.take().unwrap();
    let child_stdin = child.stdin.take().unwrap();
    spawn_stderr_reader(child.stderr.take().unwrap());

    let client = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ().serve((child_stdout, child_stdin)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Sandboxed handshake timed out"))?
    .map_err(|e| anyhow::anyhow!("Sandboxed handshake failed: {e}"))?;

    eprintln!("  ✅ Sandboxed handshake succeeded");
    run_tool_tests(&client).await?;
    client.cancel().await?;
    eprintln!("--- Sandboxed stdio: ALL PASSED ---");
    Ok(())
}

// ==========================================================================
// Test 5: Sandboxed stdio WITHOUT workspace — proves the sandbox blocks
// the MCP server when workspace_path is None (the bug we fixed).
// ==========================================================================
#[tokio::test]
async fn test_sandboxed_stdio_without_workspace_fails() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    check_mock_server_built();

    if cfg!(not(target_os = "macos")) {
        eprintln!("--- Skipping sandboxed test (not macOS) ---");
        return Ok(());
    }

    let entry = mock_server_entry();

    // Build a sandbox policy WITHOUT workspace — this is what the bug produced.
    let mut builder = hive_sandbox::SandboxPolicy::builder().network(true);
    // NO workspace added!
    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }
    builder = builder.allow_read_write(std::env::temp_dir());
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if dir.starts_with(&home) {
                    builder = builder.allow_read(&dir);
                }
            }
        }
    }
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }
    let policy = builder.build();

    let full_cmd = format!("node {} --dashboard-port 0", entry.display());
    let sandboxed = hive_sandbox::sandbox_command(&full_cmd, &policy).unwrap();
    let (program, args, _temp_files) = match sandboxed {
        hive_sandbox::SandboxedCommand::Wrapped { program, args, _temp_files } => {
            (program, args, _temp_files)
        }
        _ => {
            eprintln!("--- Skipping: sandbox-exec not available ---");
            return Ok(());
        }
    };

    eprintln!("--- Sandboxed stdio (WITHOUT workspace — should fail) ---");

    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn sandboxed node");

    let child_stdout = child.stdout.take().unwrap();
    let child_stdin = child.stdin.take().unwrap();

    // Capture stderr to verify the EPERM error
    let stderr_handle = child.stderr.take().unwrap();
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = String::new();
        let mut reader = tokio::io::BufReader::new(stderr_handle);
        reader.read_to_string(&mut buf).await.ok();
        buf
    });

    // The handshake should fail because the sandbox blocks reading the
    // server's own files (node_modules, etc.) under /Users.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        ().serve((child_stdout, child_stdin)),
    )
    .await;

    let stderr_output = stderr_task.await.unwrap_or_default();

    match result {
        Err(_) => {
            eprintln!("  ✅ Correctly timed out (server couldn't start)");
        }
        Ok(Err(_)) => {
            eprintln!("  ✅ Correctly failed handshake");
        }
        Ok(Ok(_)) => {
            // If it somehow succeeded, that means the sandbox didn't block.
            // This is the bug scenario.
            panic!(
                "Sandbox should have blocked the MCP server without workspace path!\n\
                 stderr: {}",
                stderr_output
            );
        }
    }

    assert!(
        stderr_output.contains("EPERM") || stderr_output.contains("Operation not permitted"),
        "Expected EPERM in stderr, got: {}",
        stderr_output
    );
    eprintln!("  ✅ Confirmed EPERM error in stderr");
    eprintln!("--- Sandboxed without workspace: CORRECTLY FAILED ---");
    Ok(())
}
