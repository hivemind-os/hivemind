use crate::policy::SandboxPolicy;
use crate::{SandboxError, SandboxedCommand};

/// Noop sandbox — returns [`SandboxedCommand::Passthrough`].
#[allow(dead_code)]
pub fn sandbox_command(
    _command: &str,
    _policy: &SandboxPolicy,
) -> Result<SandboxedCommand, SandboxError> {
    Ok(SandboxedCommand::Passthrough)
}
