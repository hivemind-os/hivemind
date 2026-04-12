//! Standalone test daemon for Playwright integration tests.
//!
//! Reads a JSON config from a file path (first argument) or stdin, starts a
//! `TestDaemon` with the specified scripted LLM responses, and writes a JSON
//! blob with `{ "base_url": "...", "auth_token": "..." }` to stdout.
//!
//! The daemon runs until it receives a SIGTERM, stdin EOF, or the process is
//! otherwise killed.

use hive_model::ModelRouter;
use hive_test_utils::{ScriptedProvider, TestDaemon};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::signal;

/// Configuration for the test daemon, read from JSON.
#[derive(Deserialize)]
struct DaemonConfig {
    /// Scripted LLM rules: needle → list of responses.
    #[serde(default)]
    rules: Vec<ScriptedRule>,
    /// Default fallback responses.
    #[serde(default)]
    default_responses: Vec<ScriptedResponseDef>,
}

#[derive(Deserialize)]
struct ScriptedRule {
    /// Substring to match in the system prompt or user prompt.
    needle: String,
    /// Ordered responses to return when the needle matches.
    responses: Vec<ScriptedResponseDef>,
}

#[derive(Deserialize)]
struct ScriptedResponseDef {
    /// Text content of the response.
    #[serde(default)]
    content: String,
    /// Tool calls to include in the response.
    #[serde(default)]
    tool_calls: Vec<ToolCallDef>,
}

#[derive(Deserialize)]
struct ToolCallDef {
    id: String,
    name: String,
    arguments: serde_json::Value,
}

fn build_provider(config: &DaemonConfig) -> ScriptedProvider {
    let mut provider = ScriptedProvider::new("mock", "test-model");
    for rule in &config.rules {
        let responses: Vec<_> = rule
            .responses
            .iter()
            .map(|r| {
                if r.tool_calls.is_empty() {
                    ScriptedProvider::text_response("mock", "test-model", &r.content)
                } else {
                    let calls: Vec<(String, String, serde_json::Value)> = r
                        .tool_calls
                        .iter()
                        .map(|tc| (tc.id.clone(), tc.name.clone(), tc.arguments.clone()))
                        .collect();
                    ScriptedProvider::multi_tool_response("mock", "test-model", calls)
                }
            })
            .collect();
        provider = provider.on_system_contains(&rule.needle, responses);
    }
    if !config.default_responses.is_empty() {
        let defaults: Vec<_> = config
            .default_responses
            .iter()
            .map(|r| ScriptedProvider::text_response("mock", "test-model", &r.content))
            .collect();
        provider = provider.default_responses(defaults);
    }
    provider
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Read config from file arg or stdin.
    let config: DaemonConfig = if let Some(path) = std::env::args().nth(1) {
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str(&contents)?
    } else {
        let stdin = std::io::read_to_string(std::io::stdin())?;
        if stdin.trim().is_empty() {
            DaemonConfig {
                rules: vec![],
                default_responses: vec![ScriptedResponseDef {
                    content: "I am the default test agent.".to_string(),
                    tool_calls: vec![],
                }],
            }
        } else {
            serde_json::from_str(&stdin)?
        }
    };

    let provider = build_provider(&config);
    let mut router = ModelRouter::new();
    router.register_provider(provider);

    let daemon = TestDaemon::builder().with_model_router(Arc::new(router)).spawn().await?;

    // Write connection info to stdout.
    let info = json!({
        "base_url": daemon.base_url,
        "auth_token": "test-token",
    });
    println!("{}", serde_json::to_string(&info)?);

    // Wait for SIGTERM or Ctrl-C.
    signal::ctrl_c().await?;
    daemon.stop().await?;
    Ok(())
}
