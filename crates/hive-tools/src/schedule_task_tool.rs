use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::permissions::SessionPermissions;
use hive_contracts::{
    CreateTaskRequest, ListTasksFilter, ScheduledTask, TaskAction, ToolAnnotations, ToolApproval,
    ToolDefinition, UpdateTaskRequest,
};
use hive_scheduler::SchedulerService;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool that lets agents manage scheduled tasks (create, list, cancel, update).
/// Calls the SchedulerService directly (no HTTP round-trip).
///
/// When creating `CallTool` actions, validates:
/// 1. The tool_id resolves to a known tool (handles sanitized/prefixed IDs)
/// 2. The tool is in the session's allowed tool set (privilege check)
/// 3. If the tool requires `Ask` approval, requests user confirmation first
pub struct ScheduleTaskTool {
    definition: ToolDefinition,
    scheduler: Arc<SchedulerService>,
    session_id: Option<String>,
    /// The session's allowed tool patterns (e.g. `["*"]` or `["comm.send_external_message", ...]`).
    allowed_tools: Vec<String>,
    /// Per-session permissions for checking tool approval overrides.
    permissions: Option<Arc<Mutex<SessionPermissions>>>,
}

impl ScheduleTaskTool {
    pub fn new(
        scheduler: Arc<SchedulerService>,
        session_id: Option<String>,
        allowed_tools: Vec<String>,
        permissions: Option<Arc<Mutex<SessionPermissions>>>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.schedule_task".to_string(),
                name: "Schedule Task".to_string(),
                description: concat!(
                    "Manage scheduled tasks. Tasks can run once (immediately), at a specific ",
                    "time, or on a cron schedule.\n\n",
                    "Operations:\n",
                    "- create: Create a new scheduled task\n",
                    "- list: List your scheduled tasks\n",
                    "- cancel: Cancel a pending/running task\n",
                    "- update: Update a task's name, description, schedule, or action\n",
                    "- delete: Permanently delete a task\n",
                    "- get_runs: Get execution history for a task\n\n",
                    "Action types:\n",
                    "- send_message: Inject a message into a session\n",
                    "- http_webhook: Fire an HTTP request\n",
                    "- emit_event: Publish an event to a topic\n",
                    "- invoke_agent: Spawn an agent with a persona to run a task\n",
                    "- call_tool: Invoke a registered tool with arguments (use the tool's canonical ID, e.g. \"comm.send_external_message\"). ",
                    "Only tools available in your current session can be scheduled. ",
                    "If the tool requires user approval, you will be asked to confirm before scheduling.\n",
                    "- composite_action: Run multiple actions in sequence\n",
                    "- launch_workflow: Launch a workflow by definition name with optional inputs and trigger step ID\n\n",
                    "If you receive an `approval_required` response when creating a call_tool action, ",
                    "ask the user for permission, then retry with `user_approved: true`."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["create", "list", "cancel", "update", "delete", "get_runs"],
                            "description": "The operation to perform"
                        },
                        "task_id": {
                            "type": "string",
                            "description": "Task ID (required for cancel, update, delete, get_runs)"
                        },
                        "name": {
                            "type": "string",
                            "description": "Task name (required for create, optional for update)"
                        },
                        "description": {
                            "type": "string",
                            "description": "Task description (optional)"
                        },
                        "schedule": {
                            "type": "object",
                            "description": "Task schedule. Use {\"type\":\"once\"} for immediate, {\"type\":\"scheduled\",\"run_at_ms\":TIMESTAMP_MS} for a specific time, or {\"type\":\"cron\",\"expression\":\"...\"} (7-field cron: sec min hour dom month dow year)"
                        },
                        "action": {
                            "type": "object",
                            "description": concat!(
                                "Task action. Supported types:\n",
                                "- {\"type\":\"send_message\",\"session_id\":\"...\",\"content\":\"...\"} — inject a message into a session\n",
                                "- {\"type\":\"http_webhook\",\"url\":\"...\",\"method\":\"POST\",\"body\":\"...\",\"headers\":{\"Authorization\":\"Bearer ...\"}} — fire an HTTP webhook (headers optional)\n",
                                "- {\"type\":\"emit_event\",\"topic\":\"...\",\"payload\":{}} — publish an event\n",
                                "- {\"type\":\"invoke_agent\",\"persona_id\":\"...\",\"task\":\"...\"} — spawn an agent (optional: friendly_name, timeout_secs, permissions)\n",
                                "- {\"type\":\"call_tool\",\"tool_id\":\"...\",\"arguments\":{}} — invoke a tool directly (use canonical ID like \"comm.send_external_message\")\n",
                                "- {\"type\":\"composite_action\",\"actions\":[...],\"stop_on_failure\":false} — run multiple actions in sequence"
                            )
                        },
                        "user_approved": {
                            "type": "boolean",
                            "description": "Set to true after receiving an approval_required response and getting user confirmation"
                        }
                    },
                    "required": ["operation"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Schedule Task".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
            scheduler,
            session_id,
            allowed_tools,
            permissions,
        }
    }
}

impl Tool for ScheduleTaskTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let operation = input.get("operation").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `operation`".to_string())
            })?;

            match operation {
                "create" => self.create_task(&input),
                "list" => self.list_tasks(),
                "cancel" => self.cancel_task(&input),
                "update" => self.update_task(&input),
                "delete" => self.delete_task(&input),
                "get_runs" => self.get_runs(&input),
                other => Err(ToolError::InvalidInput(format!(
                    "unknown operation `{other}`. Use: create, list, cancel, update, delete, get_runs"
                ))),
            }
        })
    }
}

impl ScheduleTaskTool {
    /// Verify the caller owns the task. Returns the task if owned, error if not.
    fn verify_ownership(&self, task_id: &str) -> Result<ScheduledTask, ToolError> {
        let task = self
            .scheduler
            .get_task(task_id)
            .map_err(|e| ToolError::ExecutionFailed(format!("task not found: {e}")))?;

        // If this tool has no session_id (e.g. admin context), allow all operations
        if let Some(ref my_session) = self.session_id {
            if task.owner_session_id.as_deref() != Some(my_session.as_str()) {
                return Err(ToolError::ExecutionFailed(format!(
                    "you do not own task `{task_id}`. You can only manage tasks created by your session."
                )));
            }
        }

        Ok(task)
    }

    fn create_task(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("create requires `name`".to_string()))?;

        let schedule = input
            .get("schedule")
            .ok_or_else(|| ToolError::InvalidInput("create requires `schedule`".to_string()))?;

        let action = input
            .get("action")
            .ok_or_else(|| ToolError::InvalidInput("create requires `action`".to_string()))?;

        let schedule = serde_json::from_value(schedule.clone())
            .map_err(|e| ToolError::InvalidInput(format!("invalid schedule: {e}")))?;
        let mut action: TaskAction = serde_json::from_value(action.clone())
            .map_err(|e| ToolError::InvalidInput(format!("invalid action: {e}")))?;

        let user_approved = input.get("user_approved").and_then(|v| v.as_bool()).unwrap_or(false);

        // Validate and resolve CallTool actions (privilege + approval checks)
        if let Some(approval_result) = self.check_action_approval(&mut action, user_approved)? {
            return Ok(approval_result);
        }

        let request = CreateTaskRequest {
            name: name.to_string(),
            description: input.get("description").and_then(|v| v.as_str()).map(String::from),
            schedule,
            action,
            owner_session_id: self.session_id.clone(),
            owner_agent_id: None,
            max_retries: input.get("max_retries").and_then(|v| v.as_u64()).map(|v| v as u32),
            retry_delay_ms: input.get("retry_delay_ms").and_then(|v| v.as_u64()),
        };

        let task = self
            .scheduler
            .create_task(request)
            .map_err(|e| ToolError::ExecutionFailed(format!("create task failed: {e}")))?;

        let output = serde_json::to_value(&task).unwrap_or_else(|_| json!({"id": task.id}));
        Ok(ToolResult { output, data_class: DataClass::Internal })
    }

    /// Check approval for CallTool actions. Returns:
    /// - `Ok(Some(result))` if the user needs to approve (returns approval_required response)
    /// - `Ok(None)` if the action is approved and can proceed
    /// - `Err(...)` if the action is denied or invalid
    ///
    /// Also resolves sanitized tool IDs to canonical form in-place.
    fn check_action_approval(
        &self,
        action: &mut TaskAction,
        user_approved: bool,
    ) -> Result<Option<ToolResult>, ToolError> {
        match action {
            TaskAction::CallTool { tool_id, .. } => {
                self.check_single_tool_approval(tool_id, user_approved)
            }
            TaskAction::CompositeAction { actions, .. } => {
                let mut needs_approval = Vec::new();
                for sub in actions.iter_mut() {
                    if let TaskAction::CallTool { tool_id, .. } = sub {
                        if let Some(result) =
                            self.check_single_tool_approval(tool_id, user_approved)?
                        {
                            // Collect the tool_id that needs approval
                            if let Some(tid) = result.output.get("tool_id").and_then(|v| v.as_str())
                            {
                                needs_approval.push(tid.to_string());
                            }
                        }
                    }
                }
                if !needs_approval.is_empty() {
                    Ok(Some(ToolResult {
                        output: json!({
                            "status": "approval_required",
                            "tools": needs_approval,
                            "message": format!(
                                "The following tools require user approval before they can be scheduled: {}. \
                                 Please ask the user for permission, then retry with `user_approved: true`.",
                                needs_approval.join(", ")
                            )
                        }),
                        data_class: DataClass::Internal,
                    }))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None), // Non-tool actions don't need approval
        }
    }

    /// Check privilege and approval for a single tool_id.
    /// Resolves the tool_id to canonical form in-place.
    fn check_single_tool_approval(
        &self,
        tool_id: &mut String,
        user_approved: bool,
    ) -> Result<Option<ToolResult>, ToolError> {
        // 1. Resolve sanitized/prefixed tool_id to canonical form
        if let Some(executor) = self.scheduler.tool_executor() {
            if let Some(canonical) = executor.resolve_tool_id(tool_id) {
                *tool_id = canonical;
            }
        }

        // 2. Privilege check: is the tool in the session's allowed set?
        if !self.is_tool_allowed(tool_id) {
            return Err(ToolError::ExecutionFailed(format!(
                "tool `{tool_id}` is not available in your current session. \
                 You can only schedule tools you have access to."
            )));
        }

        // 3. Approval check
        let effective_approval = self.effective_approval(tool_id);
        match effective_approval {
            ToolApproval::Auto => Ok(None), // proceed
            ToolApproval::Ask => {
                if user_approved {
                    Ok(None) // user already approved
                } else {
                    Ok(Some(ToolResult {
                        output: json!({
                            "status": "approval_required",
                            "tool_id": tool_id,
                            "approval_level": "ask",
                            "message": format!(
                                "The tool `{tool_id}` requires user approval. \
                                 Scheduled tasks run without user interaction, so approval must be \
                                 granted now. Ask the user for permission, then retry with \
                                 `user_approved: true`."
                            )
                        }),
                        data_class: DataClass::Internal,
                    }))
                }
            }
            ToolApproval::Deny => Err(ToolError::ExecutionFailed(format!(
                "tool `{tool_id}` is denied by session permissions and cannot be scheduled."
            ))),
        }
    }

    /// Check if a tool_id is in the session's allowed tool set.
    /// Mirrors the logic in `ToolRegistry::filtered()`.
    fn is_tool_allowed(&self, tool_id: &str) -> bool {
        // core.* and mcp.* are always allowed (same as ToolRegistry::filtered)
        if tool_id.starts_with("core.") || tool_id.starts_with("mcp.") {
            return true;
        }
        // "*" means all tools allowed
        if self.allowed_tools.iter().any(|t| t == "*") {
            return true;
        }
        self.allowed_tools.iter().any(|t| t == tool_id)
    }

    /// Determine the effective approval level for a tool.
    /// Session permissions override, then fall back to tool definition default.
    fn effective_approval(&self, tool_id: &str) -> ToolApproval {
        // Check session permissions first
        if let Some(ref perms) = self.permissions {
            let perms = perms.lock();
            if let Some(decision) = perms.resolve(tool_id, "*") {
                return decision;
            }
        }

        // Fall back to tool definition's default approval
        if let Some(executor) = self.scheduler.tool_executor() {
            if let Some(approval) = executor.get_tool_approval(tool_id) {
                return approval;
            }
        }

        // If we can't determine (no executor configured), default to Auto.
        // The scheduler's validate_action will catch invalid tools at execution time.
        ToolApproval::Auto
    }

    fn list_tasks(&self) -> Result<ToolResult, ToolError> {
        let filter = ListTasksFilter { session_id: self.session_id.clone(), ..Default::default() };
        let tasks = self
            .scheduler
            .list_tasks_filtered(&filter)
            .map_err(|e| ToolError::ExecutionFailed(format!("list tasks failed: {e}")))?;
        let output = serde_json::to_value(&tasks).unwrap_or_else(|_| json!([]));
        Ok(ToolResult { output, data_class: DataClass::Internal })
    }

    fn cancel_task(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("cancel requires `task_id`".to_string()))?;

        self.verify_ownership(task_id)?;

        let task = self
            .scheduler
            .cancel_task(task_id)
            .map_err(|e| ToolError::ExecutionFailed(format!("cancel task failed: {e}")))?;

        let output = serde_json::to_value(&task).unwrap_or_else(|_| json!({"cancelled": task_id}));
        Ok(ToolResult { output, data_class: DataClass::Internal })
    }

    fn update_task(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("update requires `task_id`".to_string()))?;

        self.verify_ownership(task_id)?;

        let schedule = match input.get("schedule") {
            Some(v) => Some(
                serde_json::from_value(v.clone())
                    .map_err(|e| ToolError::InvalidInput(format!("invalid schedule: {e}")))?,
            ),
            None => None,
        };
        let mut action: Option<TaskAction> = match input.get("action") {
            Some(v) => Some(
                serde_json::from_value(v.clone())
                    .map_err(|e| ToolError::InvalidInput(format!("invalid action: {e}")))?,
            ),
            None => None,
        };

        // Require approval for privileged actions (same as create_task).
        if let Some(ref mut act) = action {
            if let Some(approval_result) = self.check_action_approval(
                act,
                input.get("user_approved").and_then(|v| v.as_bool()).unwrap_or(false),
            )? {
                return Ok(approval_result);
            }
        }

        let max_retries = input.get("max_retries").map(|v| v.as_u64().map(|n| n as u32));
        let retry_delay_ms = input.get("retry_delay_ms").map(|v| v.as_u64());

        let request = UpdateTaskRequest {
            name: input.get("name").and_then(|v| v.as_str()).map(String::from),
            description: input.get("description").and_then(|v| v.as_str()).map(String::from),
            schedule,
            action,
            max_retries,
            retry_delay_ms,
        };

        let task = self
            .scheduler
            .update_task(task_id, request)
            .map_err(|e| ToolError::ExecutionFailed(format!("update task failed: {e}")))?;

        let output = serde_json::to_value(&task).unwrap_or_else(|_| json!({"updated": task_id}));
        Ok(ToolResult { output, data_class: DataClass::Internal })
    }

    fn delete_task(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("delete requires `task_id`".to_string()))?;

        self.verify_ownership(task_id)?;

        self.scheduler
            .delete_task(task_id)
            .map_err(|e| ToolError::ExecutionFailed(format!("delete task failed: {e}")))?;

        Ok(ToolResult { output: json!({"deleted": task_id}), data_class: DataClass::Internal })
    }

    fn get_runs(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let task_id = input
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("get_runs requires `task_id`".to_string()))?;

        self.verify_ownership(task_id)?;

        let runs = self
            .scheduler
            .list_task_runs(task_id)
            .map_err(|e| ToolError::ExecutionFailed(format!("get runs failed: {e}")))?;

        let output = serde_json::to_value(&runs).unwrap_or_else(|_| json!([]));
        Ok(ToolResult { output, data_class: DataClass::Internal })
    }
}
