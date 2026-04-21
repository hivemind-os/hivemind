use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use hive_contracts::TaskAction;
use hive_core::EventBus;
use serde_json::Value;

use crate::{SchedulerAgentRunner, SchedulerToolExecutor};

// ---------------------------------------------------------------------------
// ActionExecutor trait
// ---------------------------------------------------------------------------

/// Trait for executing a specific type of scheduler action.
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    /// Execute the action, returning an optional result value.
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String>;
}

// ---------------------------------------------------------------------------
// ActionRegistry
// ---------------------------------------------------------------------------

pub struct ActionRegistry {
    executors: parking_lot::RwLock<HashMap<String, Arc<dyn ActionExecutor>>>,
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self { executors: parking_lot::RwLock::new(HashMap::new()) }
    }

    pub fn register(&self, action_type: &str, executor: Arc<dyn ActionExecutor>) {
        self.executors.write().insert(action_type.to_string(), executor);
    }

    pub async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let action_type = action.type_name();
        let executor = self.executors.read().get(action_type).cloned();
        if let Some(executor) = executor {
            executor.execute(action).await
        } else {
            Err(format!("No executor registered for action type: {action_type}"))
        }
    }
}

// ---------------------------------------------------------------------------
// EmitEventExecutor
// ---------------------------------------------------------------------------

pub(crate) struct EmitEventExecutor {
    pub(crate) event_bus: EventBus,
}

#[async_trait]
impl ActionExecutor for EmitEventExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::EmitEvent { topic, payload } = action else {
            return Err("EmitEventExecutor received non-EmitEvent action".to_string());
        };
        match self.event_bus.publish(topic.as_str(), "scheduler", payload.clone()) {
            Ok(_) => Ok(None),
            Err(_) => Ok(None), // No subscribers — not a failure
        }
    }
}

// ---------------------------------------------------------------------------
// SendMessageExecutor
// ---------------------------------------------------------------------------

pub(crate) struct SendMessageExecutor {
    pub(crate) http_client: reqwest::Client,
    pub(crate) daemon_addr: String,
    pub(crate) auth_token: Option<String>,
}

#[async_trait]
impl ActionExecutor for SendMessageExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::SendMessage { session_id, content } = action else {
            return Err("SendMessageExecutor received non-SendMessage action".to_string());
        };
        let url =
            format!("http://{}/api/v1/chat/sessions/{}/messages", self.daemon_addr, session_id);
        let mut request =
            self.http_client.post(&url).json(&serde_json::json!({ "content": content }));
        if let Some(ref token) = self.auth_token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(%session_id, "SendMessage delivered");
                Ok(None)
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Err(format!("SendMessage failed with status {status}: {body}"))
            }
            Err(e) => Err(format!("SendMessage request failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// HttpWebhookExecutor
// ---------------------------------------------------------------------------

pub(crate) struct HttpWebhookExecutor {
    pub(crate) http_client: reqwest::Client,
}

#[async_trait]
impl ActionExecutor for HttpWebhookExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::HttpWebhook { url, method, body, headers } = action else {
            return Err("HttpWebhookExecutor received non-HttpWebhook action".to_string());
        };

        // SSRF protection: block requests to private/internal addresses.
        #[cfg(not(feature = "allow-localhost"))]
        crate::validate_webhook_url(url)?;

        let request = match method.to_uppercase().as_str() {
            "GET" => self.http_client.get(url),
            "POST" => self.http_client.post(url),
            "PUT" => self.http_client.put(url),
            "DELETE" => self.http_client.delete(url),
            "PATCH" => self.http_client.patch(url),
            "HEAD" => self.http_client.head(url),
            other => {
                return Err(format!(
                "unsupported HTTP method: {other}. Allowed: GET, POST, PUT, DELETE, PATCH, HEAD"
            ))
            }
        };
        let has_content_type = headers
            .as_ref()
            .is_some_and(|hdrs| hdrs.keys().any(|k| k.eq_ignore_ascii_case("content-type")));
        let request = if let Some(hdrs) = headers {
            let mut r = request;
            for (k, v) in hdrs {
                r = r.header(k.as_str(), v.as_str());
            }
            r
        } else {
            request
        };
        let request = if let Some(body) = body {
            let r = request.body(body.clone());
            if has_content_type {
                r
            } else {
                r.header("content-type", "application/json")
            }
        } else {
            request
        };
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                let body = resp.text().await.unwrap_or_default();
                let result = serde_json::from_str::<Value>(&body).ok();
                Ok(result)
            }
            Ok(resp) => {
                let status = resp.status();
                Err(format!("HttpWebhook failed with status {status}"))
            }
            Err(e) => Err(format!("HttpWebhook request failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// InvokeAgentExecutor
// ---------------------------------------------------------------------------

pub(crate) struct InvokeAgentExecutor {
    pub(crate) agent_runner: Option<Arc<dyn SchedulerAgentRunner>>,
}

#[async_trait]
impl ActionExecutor for InvokeAgentExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::InvokeAgent {
            persona_id,
            task,
            friendly_name,
            async_exec,
            timeout_secs,
            permissions,
        } = action
        else {
            return Err("InvokeAgentExecutor received non-InvokeAgent action".to_string());
        };
        let runner = self.agent_runner.as_ref().ok_or_else(|| {
            "InvokeAgent action unavailable: no agent runner configured".to_string()
        })?;
        let timeout = timeout_secs.unwrap_or(300);
        let result = runner
            .run_agent(
                persona_id,
                task,
                friendly_name.clone(),
                *async_exec,
                timeout,
                permissions.clone(),
            )
            .await?;
        Ok(result.map(Value::String))
    }
}

// ---------------------------------------------------------------------------
// CallToolExecutor
// ---------------------------------------------------------------------------

pub(crate) struct CallToolExecutor {
    pub(crate) tool_executor: Option<Arc<dyn SchedulerToolExecutor>>,
}

#[async_trait]
impl ActionExecutor for CallToolExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::CallTool { tool_id, arguments } = action else {
            return Err("CallToolExecutor received non-CallTool action".to_string());
        };
        let executor = self.tool_executor.as_ref().ok_or_else(|| {
            "CallTool action unavailable: no tool executor configured".to_string()
        })?;
        let output = executor.execute_tool(tool_id, arguments.clone()).await?;
        Ok(Some(output))
    }
}

// ---------------------------------------------------------------------------
// LaunchWorkflowExecutor
// ---------------------------------------------------------------------------

pub(crate) struct LaunchWorkflowExecutor {
    pub(crate) http_client: reqwest::Client,
    pub(crate) daemon_addr: String,
    pub(crate) auth_token: Option<String>,
}

#[async_trait]
impl ActionExecutor for LaunchWorkflowExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::LaunchWorkflow { definition, version, inputs, trigger_step_id } = action
        else {
            return Err("LaunchWorkflowExecutor received non-LaunchWorkflow action".to_string());
        };
        let url = format!("http://{}/api/v1/workflows/instances", self.daemon_addr);
        let mut body = serde_json::json!({
            "definition": definition,
            "inputs": inputs,
            "parent_session_id": "scheduler",
        });
        if let Some(v) = version {
            body["version"] = serde_json::json!(v);
        }
        if let Some(ts) = trigger_step_id {
            body["trigger_step_id"] = serde_json::json!(ts);
        }
        let mut request = self.http_client.post(&url).json(&body);
        if let Some(ref token) = self.auth_token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                let result: Value = resp.json().await.unwrap_or_default();
                tracing::info!(definition, "LaunchWorkflow succeeded");
                Ok(Some(result))
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Err(format!("LaunchWorkflow failed with status {status}: {body}"))
            }
            Err(e) => Err(format!("LaunchWorkflow request failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// CompositeActionExecutor
// ---------------------------------------------------------------------------

pub(crate) struct CompositeActionExecutor {
    pub(crate) registry: Arc<ActionRegistry>,
    pub(crate) timeout_secs: u64,
}

impl CompositeActionExecutor {
    async fn execute_actions(
        &self,
        actions: &[TaskAction],
        stop_on_failure: bool,
    ) -> Result<Option<Value>, String> {
        let mut results: Vec<Value> = Vec::with_capacity(actions.len());
        for (i, sub_action) in actions.iter().enumerate() {
            match self.registry.execute(sub_action).await {
                Ok(v) => {
                    results.push(serde_json::json!({
                        "index": i,
                        "status": "success",
                        "result": v,
                    }));
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "index": i,
                        "status": "failure",
                        "error": e,
                    }));
                    if stop_on_failure {
                        return Err(format!("CompositeAction stopped at action {i}: {e}"));
                    }
                }
            }
        }
        let failure_count = results
            .iter()
            .filter(|r| r.get("status").and_then(|s| s.as_str()) == Some("failure"))
            .count();
        if failure_count > 0 {
            let results_json = serde_json::to_string(&results).unwrap_or_default();
            return Err(format!(
                "CompositeAction completed with {failure_count}/{} action(s) failed. Results: {results_json}",
                actions.len()
            ));
        }
        Ok(Some(Value::Array(results)))
    }
}

#[async_trait]
impl ActionExecutor for CompositeActionExecutor {
    async fn execute(&self, action: &TaskAction) -> Result<Option<Value>, String> {
        let TaskAction::CompositeAction { actions, stop_on_failure } = action else {
            return Err("CompositeActionExecutor received non-CompositeAction action".to_string());
        };

        let timeout_duration = std::time::Duration::from_secs(self.timeout_secs);
        match tokio::time::timeout(
            timeout_duration,
            self.execute_actions(actions, *stop_on_failure),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!("CompositeAction timed out after {}s", self.timeout_secs)),
        }
    }
}
