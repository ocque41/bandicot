use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::types::{CapabilityMode, GraphSpec, NodeId, NodeStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompensationStepStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompensationStep {
    pub source_node_id: NodeId,
    pub compensation_node_id: NodeId,
    pub status: CompensationStepStatus,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompensationPlanStatus {
    Pending,
    Running,
    Completed,
    Failed,
    ManualInterventionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompensationPlan {
    pub steps: Vec<CompensationStep>,
    pub cursor: usize,
    pub status: CompensationPlanStatus,
}

impl CompensationPlan {
    pub fn current(&self) -> Option<&CompensationStep> {
        self.steps.get(self.cursor)
    }

    pub fn mark_running(&mut self, attempt: u32) {
        self.status = CompensationPlanStatus::Running;
        if let Some(step) = self.steps.get_mut(self.cursor) {
            step.status = CompensationStepStatus::Running;
            step.attempt = attempt;
        }
    }

    pub fn mark_completed(&mut self) {
        if let Some(step) = self.steps.get_mut(self.cursor) {
            step.status = CompensationStepStatus::Completed;
        }
        self.cursor = self.cursor.saturating_add(1);
        if self.cursor >= self.steps.len() {
            self.status = CompensationPlanStatus::Completed;
        }
    }

    pub fn mark_failed(&mut self) {
        if let Some(step) = self.steps.get_mut(self.cursor) {
            step.status = CompensationStepStatus::Failed;
        }
        self.status = CompensationPlanStatus::Failed;
    }

    pub fn is_complete(&self) -> bool {
        self.status == CompensationPlanStatus::Completed
    }
}

pub fn build_compensation_plan(
    spec: &GraphSpec,
    completed_in_order: &[NodeId],
) -> CompensationPlan {
    let mut seen_compensations = BTreeSet::new();
    let mut steps = Vec::new();
    for source_id in completed_in_order.iter().rev() {
        let Some(source) = spec.spec.nodes.iter().find(|node| &node.id == source_id) else {
            continue;
        };
        let side_effecting = source.effective_capability(&spec.spec.defaults)
            > CapabilityMode::ReadOnly
            || !source.external_effects.is_empty()
            || !source.write_set.is_empty();
        let Some(compensation_node_id) = source.compensation.as_ref() else {
            continue;
        };
        if side_effecting && seen_compensations.insert(compensation_node_id.clone()) {
            steps.push(CompensationStep {
                source_node_id: source.id.clone(),
                compensation_node_id: compensation_node_id.clone(),
                status: CompensationStepStatus::Pending,
                attempt: 0,
            });
        }
    }
    let status = if steps.is_empty() {
        CompensationPlanStatus::Completed
    } else {
        CompensationPlanStatus::Pending
    };
    CompensationPlan {
        steps,
        cursor: 0,
        status,
    }
}

pub fn completed_node_order_from_events(events: &[super::store::GraphEvent]) -> Vec<NodeId> {
    let mut order = Vec::new();
    for event in events {
        if let super::store::GraphEvent::NodeOutputAccepted {
            node_id,
            status: NodeStatus::Succeeded,
            ..
        } = event
            && !order.contains(node_id)
        {
            order.push(node_id.clone());
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::agent_graph::{NodeKind, build_exact_100_worker_graph};

    #[test]
    fn plan_uses_reverse_completion_order_and_deduplicates_compensators() {
        let mut graph = build_exact_100_worker_graph("compensation-plan");
        graph.spec.nodes.truncate(4);
        graph.spec.execution.max_total_nodes = 4;
        for index in 0..2 {
            graph.spec.nodes[index].capability_mode = Some(CapabilityMode::WorktreeWrite);
            graph.spec.nodes[index].write_set = vec![format!("file-{index}")];
            graph.spec.nodes[index].compensation = Some(format!("worker-{:03}", index + 2));
        }
        graph.spec.nodes[2].kind = NodeKind::Compensation;
        graph.spec.nodes[3].kind = NodeKind::Compensation;
        let plan = build_compensation_plan(
            &graph,
            &["worker-000".to_string(), "worker-001".to_string()],
        );
        assert_eq!(plan.steps[0].source_node_id, "worker-001");
        assert_eq!(plan.steps[1].source_node_id, "worker-000");
    }
}
