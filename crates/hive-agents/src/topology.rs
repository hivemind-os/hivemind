use std::collections::HashSet;

use crate::error::AgentError;
use crate::supervisor::AgentSupervisor;
use crate::types::{AgentMessage, FlowType, TopologyDef};

impl AgentSupervisor {
    /// Set up a multi-agent topology: spawn all agents and send the initial
    /// task to the entry-point agent(s).
    ///
    /// Entry points are agents that have no incoming flow edges.
    pub async fn run_topology(
        &self,
        topology: TopologyDef,
        initial_task: String,
    ) -> Result<(), AgentError> {
        if topology.agents.is_empty() {
            return Err(AgentError::TopologyError("topology has no agents".to_string()));
        }

        // 1. Spawn all agents
        for spec in &topology.agents {
            self.spawn_agent(spec.clone(), None, None, None, None).await?;
        }

        // 2. Find entry points — agents with no incoming edges
        let all_ids: HashSet<&str> = topology.agents.iter().map(|a| a.id.as_str()).collect();
        let mut targets: HashSet<&str> = HashSet::new();
        for edge in &topology.flows {
            for to in &edge.to {
                targets.insert(to.as_str());
            }
        }
        let entry_points: Vec<&str> = all_ids.difference(&targets).copied().collect();

        if entry_points.is_empty() {
            // All agents have incoming edges — pick the first agent as fallback
            let first = &topology.agents[0].id;
            self.send_to_agent(
                first,
                AgentMessage::Task { content: initial_task, from: Some("supervisor".to_string()) },
            )
            .await?;
        } else {
            // Send the initial task to every entry-point agent
            for ep in &entry_points {
                self.send_to_agent(
                    ep,
                    AgentMessage::Task {
                        content: initial_task.clone(),
                        from: Some("supervisor".to_string()),
                    },
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Validate a topology definition before running it.
    pub fn validate_topology(topology: &TopologyDef) -> Result<(), AgentError> {
        if topology.agents.is_empty() {
            return Err(AgentError::TopologyError("topology has no agents".to_string()));
        }

        let ids: HashSet<&str> = topology.agents.iter().map(|a| a.id.as_str()).collect();

        // Ensure every flow references existing agents
        for edge in &topology.flows {
            if !ids.contains(edge.from.as_str()) {
                return Err(AgentError::TopologyError(format!(
                    "flow references unknown source agent: {}",
                    edge.from
                )));
            }
            for to in &edge.to {
                if !ids.contains(to.as_str()) {
                    return Err(AgentError::TopologyError(format!(
                        "flow references unknown target agent: {to}"
                    )));
                }
            }
        }

        // Validate flow-type specific constraints
        for edge in &topology.flows {
            match edge.flow_type {
                FlowType::Pipeline => {
                    if edge.to.len() != 1 {
                        return Err(AgentError::TopologyError(format!(
                            "pipeline flow from {} must have exactly one target, got {}",
                            edge.from,
                            edge.to.len()
                        )));
                    }
                }
                FlowType::FanOut => {
                    if edge.to.len() < 2 {
                        return Err(AgentError::TopologyError(format!(
                            "fan-out flow from {} must have at least 2 targets, got {}",
                            edge.from,
                            edge.to.len()
                        )));
                    }
                }
                FlowType::FanIn | FlowType::Feedback => {
                    // No special constraints
                }
            }
        }

        Ok(())
    }
}
