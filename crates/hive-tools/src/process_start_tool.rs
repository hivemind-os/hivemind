use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    DetectedShells, SandboxConfig, ToolAnnotations, ToolApproval, ToolDefinition,
};
use hive_process::{ProcessManager, ProcessOwner};
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// Spawn a background process in a PTY.
pub struct ProcessStartTool {
    definition: ToolDefinition,
    manager: Arc<ProcessManager>,
    env_vars: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    sandbox_config: Arc<parking_lot::RwLock<SandboxConfig>>,
    owner: ProcessOwner,
    /// Default working directory (workspace root) used when the caller omits `working_dir`.
    default_dir: Option<std::path::PathBuf>,
    /// Detected shells available on the system.
    detected_shells: Arc<DetectedShells>,
}

impl ProcessStartTool {
    pub fn new(
        manager: Arc<ProcessManager>,
        env_vars: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
        sandbox_config: Arc<parking_lot::RwLock<SandboxConfig>>,
        owner: ProcessOwner,
        default_dir: Option<std::path::PathBuf>,
        detected_shells: Option<Arc<DetectedShells>>,
    ) -> Self {
        let shells = detected_shells.unwrap_or_else(|| Arc::new(DetectedShells::default()));
        let shell_summary = shells.description_summary();
        let available_names = shells.available_names().join(", ");

        let description = format!(
            "Start a long-running background process in a PTY. \
             Returns a process ID that can be used with process.status, \
             process.write, process.kill, and process.list to manage it. \
             Suitable for dev servers, watchers, builds, and other \
             long-running commands. {}. \
             Use the 'shell' parameter to select a specific shell (available: {}).",
            shell_summary, available_names
        );

        Self {
            definition: ToolDefinition {
                id: "process.start".to_string(),
                name: "Start background process".to_string(),
                description,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute in the background."
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "Optional working directory for the command."
                        },
                        "env": {
                            "type": "object",
                            "description": "Optional environment variables as key-value pairs.",
                            "additionalProperties": { "type": "string" }
                        },
                        "buffer_size": {
                            "type": "number",
                            "description": "Output ring buffer size in bytes (default: 65536)."
                        },
                        "shell": {
                            "type": "string",
                            "description": format!("Optional shell to use for execution (available: {}). Defaults to '{}'.", available_names, shells.default_shell)
                        }
                    },
                    "required": ["command"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "process_id": { "type": "string" },
                        "pid": { "type": "number" }
                    }
                })),
                channel_class: ChannelClass::LocalOnly,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Start background process".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
            manager,
            env_vars,
            sandbox_config,
            owner,
            default_dir,
            detected_shells: shells,
        }
    }

    fn build_sandbox_policy(&self, working_dir: Option<&str>) -> hive_sandbox::SandboxPolicy {
        let cfg = self.sandbox_config.read().clone();
        let mut builder = hive_sandbox::SandboxPolicy::builder().network(cfg.allow_network);

        if let Some(dir) = working_dir {
            builder = builder.allow_read_write(dir);
        }
        builder = builder.allow_read_write(std::env::temp_dir());

        for p in hive_sandbox::default_system_read_paths() {
            builder = builder.allow_read(p);
        }

        // PATH entries under $HOME (nvm, pyenv, conda, etc.)
        builder = crate::allow_home_path_entries(builder);

        // Read-write: user HOME directory for build tool caches (cargo, npm,
        // dotnet, go, etc.). Sensitive sub-directories are denied below.
        if let Some(home) = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
        {
            builder = builder.allow_read_write(&home);
        }

        // HiveMind OS home (managed runtimes)
        if let Some(hivemind_home) = crate::resolve_hivemind_home() {
            builder = builder.allow_read(hivemind_home);
        }

        for p in hive_sandbox::default_denied_paths() {
            builder = builder.deny(p);
        }
        for p in &cfg.extra_read_paths {
            builder = builder.allow_read(p);
        }
        for p in &cfg.extra_write_paths {
            builder = builder.allow_read_write(p);
        }
        for (k, v) in self.env_vars.read().iter() {
            if !crate::is_blocked_env_var(k) {
                builder = builder.env(k, v);
            }
        }

        builder.build()
    }
}

impl Tool for ProcessStartTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let command = input.get("command").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `command`".to_string())
            })?;

            let working_dir = input
                .get("working_dir")
                .and_then(|v| v.as_str())
                .or_else(|| self.default_dir.as_ref().and_then(|p| p.to_str()));

            // Validate user-supplied working_dir is within the workspace
            if let Some(dir) = input.get("working_dir").and_then(|v| v.as_str()) {
                crate::validate_working_dir(dir, self.default_dir.as_deref())?;
            }

            let env = input.get("env").and_then(|v| v.as_object()).map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            });

            // Validate tool-input env vars against the security blocklist
            if let Some(ref env_map) = env {
                crate::validate_env_vars(env_map)?;
            }

            let buffer_size = input.get("buffer_size").and_then(|v| v.as_u64()).map(|v| v as usize);

            // Resolve the shell to use for this invocation.
            let shell_info = if let Some(shell_name) = input.get("shell").and_then(|v| v.as_str()) {
                self.detected_shells.find_by_name(shell_name).ok_or_else(|| {
                    let available = self.detected_shells.available_names().join(", ");
                    ToolError::InvalidInput(format!(
                        "shell '{}' is not available on this system. Available shells: {}",
                        shell_name, available
                    ))
                })?
            } else {
                self.detected_shells.default_shell_info().ok_or_else(|| {
                    ToolError::ExecutionFailed("no default shell detected".to_string())
                })?
            };

            let sandbox_enabled = self.sandbox_config.read().enabled;
            let policy =
                if sandbox_enabled { Some(self.build_sandbox_policy(working_dir)) } else { None };

            let (process_id, pid) = self
                .manager
                .spawn_sandboxed_with_shell(
                    command,
                    working_dir,
                    env.as_ref(),
                    buffer_size,
                    policy.as_ref(),
                    self.owner.clone(),
                    Some(shell_info.program()),
                    Some(shell_info.kind.command_flag()),
                )
                .map_err(ToolError::ExecutionFailed)?;

            // Grace period: if the process exits immediately with an error,
            // report it as a tool failure so the agent knows.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;

            if let Ok((info, output)) = self.manager.status(&process_id, Some(50)) {
                match &info.status {
                    hive_process::ProcessStatus::Exited { code } if *code != 0 => {
                        let snippet = if output.is_empty() {
                            String::new()
                        } else {
                            format!("\n\nOutput:\n{output}")
                        };
                        return Err(ToolError::ExecutionFailed(format!(
                            "Process exited immediately with code {code}{snippet}"
                        )));
                    }
                    hive_process::ProcessStatus::Failed { error } => {
                        return Err(ToolError::ExecutionFailed(format!(
                            "Process failed to start: {error}"
                        )));
                    }
                    _ => {}
                }
            }

            Ok(ToolResult {
                output: json!({ "process_id": process_id, "pid": pid }),
                data_class: DataClass::Internal,
            })
        })
    }
}
