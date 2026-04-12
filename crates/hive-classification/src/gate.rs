use crate::model::{ChannelClass, DataClass};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverrideAction {
    Block,
    Prompt,
    Allow,
    RedactAndSend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverridePolicy {
    pub internal: OverrideAction,
    pub confidential: OverrideAction,
    pub restricted: OverrideAction,
}

impl Default for OverridePolicy {
    fn default() -> Self {
        Self {
            internal: OverrideAction::Prompt,
            confidential: OverrideAction::Prompt,
            restricted: OverrideAction::Block,
        }
    }
}

impl OverridePolicy {
    pub fn action_for(&self, level: DataClass) -> OverrideAction {
        match level {
            DataClass::Public => OverrideAction::Allow,
            DataClass::Internal => self.internal,
            DataClass::Confidential => self.confidential,
            DataClass::Restricted => self.restricted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    Block { reason: String },
    Prompt { reason: String },
    RedactAndSend { reason: String },
}

pub fn gate(level: DataClass, channel: ChannelClass, policy: &OverridePolicy) -> GateDecision {
    if channel.allows(level) {
        return GateDecision::Allow;
    }

    let reason = format!(
        "{} data cannot cross a {} channel without an override",
        level.as_str(),
        channel.as_str()
    );

    match policy.action_for(level) {
        OverrideAction::Allow => GateDecision::Allow,
        OverrideAction::Block => GateDecision::Block { reason },
        OverrideAction::Prompt => GateDecision::Prompt { reason },
        OverrideAction::RedactAndSend => GateDecision::RedactAndSend { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_defaults_to_block() {
        let decision =
            gate(DataClass::Restricted, ChannelClass::Public, &OverridePolicy::default());
        assert!(matches!(decision, GateDecision::Block { .. }));
    }

    #[test]
    fn confidential_prompts_on_public_channels() {
        let decision =
            gate(DataClass::Confidential, ChannelClass::Public, &OverridePolicy::default());
        assert!(matches!(decision, GateDecision::Prompt { .. }));
    }

    #[test]
    fn data_that_fits_channel_is_allowed() {
        let decision =
            gate(DataClass::Internal, ChannelClass::Internal, &OverridePolicy::default());
        assert_eq!(decision, GateDecision::Allow);
    }
}
