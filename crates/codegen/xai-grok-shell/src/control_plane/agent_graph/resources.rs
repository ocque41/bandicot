use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use super::types::{
    CapabilityMode, GraphSpec, NodeId, NodeSpec, ResourceDefinition, ResourceId, ResourceKind,
    ToolName,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceTicket {
    node_id: NodeId,
    claims: BTreeMap<ResourceId, u32>,
    write_set: Vec<String>,
}

impl ResourceTicket {
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResourceError {
    #[error("resource `{resource}` is not declared")]
    UnknownResource { resource: ResourceId },
    #[error("resource `{resource}` claim `{requested}` exceeds limit `{limit}`")]
    ClaimExceedsLimit {
        resource: ResourceId,
        requested: u32,
        limit: u32,
    },
    #[error("resource `{resource}` has only `{available}` permits available")]
    InsufficientPermits {
        resource: ResourceId,
        requested: u32,
        available: u32,
    },
    #[error("write set conflict with active node `{active_node}` on `{path}`")]
    WriteSetConflict { active_node: NodeId, path: String },
    #[error("plan mode rejects write-capable node `{node_id}`")]
    PlanModeWriter { node_id: NodeId },
    #[error("worker node `{node_id}` cannot receive nested orchestration tool `{tool}`")]
    NestedOrchestrationTool { node_id: NodeId, tool: ToolName },
    #[error("worker graph must keep `{flag}` enabled")]
    WorkerIsolationDisabled { flag: &'static str },
}

impl ResourceError {
    pub fn is_contention(&self) -> bool {
        matches!(
            self,
            Self::InsufficientPermits { .. } | Self::WriteSetConflict { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct ResourceManager {
    definitions: BTreeMap<ResourceId, ResourceDefinition>,
    held: BTreeMap<ResourceId, u32>,
    active_writes: BTreeMap<NodeId, Vec<String>>,
}

impl ResourceManager {
    pub fn from_graph(spec: &GraphSpec) -> Self {
        Self {
            definitions: spec.spec.resources.clone(),
            held: BTreeMap::new(),
            active_writes: BTreeMap::new(),
        }
    }

    pub fn try_acquire(
        &mut self,
        node: &NodeSpec,
        defaults: &super::types::NodeDefaults,
        plan_mode: bool,
    ) -> Result<ResourceTicket, ResourceError> {
        let capability = node.effective_capability(defaults);
        if plan_mode && capability > CapabilityMode::ReadOnly {
            return Err(ResourceError::PlanModeWriter {
                node_id: node.id.clone(),
            });
        }

        for tool in &node.tool_allowlist {
            if node.is_model_worker() && is_nested_orchestration_tool(tool) {
                return Err(ResourceError::NestedOrchestrationTool {
                    node_id: node.id.clone(),
                    tool: tool.clone(),
                });
            }
        }

        let normalized_write_set = normalized_write_set(node);
        for path in &normalized_write_set {
            if let Some((active_node, active_path)) = self.conflicting_write(path) {
                return Err(ResourceError::WriteSetConflict {
                    active_node,
                    path: active_path,
                });
            }
        }

        let mut requested = BTreeMap::new();
        for claim in &node.resource_claims {
            let Some(definition) = self.definitions.get(&claim.resource) else {
                return Err(ResourceError::UnknownResource {
                    resource: claim.resource.clone(),
                });
            };
            let limit = match definition.kind {
                ResourceKind::CountedPermit => definition.limit,
                ResourceKind::ExclusiveLock => 1,
            };
            if claim.amount == 0 || claim.amount > limit {
                return Err(ResourceError::ClaimExceedsLimit {
                    resource: claim.resource.clone(),
                    requested: claim.amount,
                    limit,
                });
            }
            let held = self.held.get(&claim.resource).copied().unwrap_or(0);
            let available = limit.saturating_sub(held);
            if claim.amount > available {
                return Err(ResourceError::InsufficientPermits {
                    resource: claim.resource.clone(),
                    requested: claim.amount,
                    available,
                });
            }
            requested.insert(claim.resource.clone(), claim.amount);
        }

        for (resource, amount) in &requested {
            *self.held.entry(resource.clone()).or_default() += *amount;
        }
        if !normalized_write_set.is_empty() {
            self.active_writes
                .insert(node.id.clone(), normalized_write_set.clone());
        }

        Ok(ResourceTicket {
            node_id: node.id.clone(),
            claims: requested,
            write_set: normalized_write_set,
        })
    }

    pub fn release(&mut self, ticket: ResourceTicket) {
        for (resource, amount) in ticket.claims {
            if let Some(held) = self.held.get_mut(&resource) {
                *held = held.saturating_sub(amount);
                if *held == 0 {
                    self.held.remove(&resource);
                }
            }
        }
        if !ticket.write_set.is_empty() {
            self.active_writes.remove(&ticket.node_id);
        }
    }

    pub fn held_amount(&self, resource: &str) -> u32 {
        self.held.get(resource).copied().unwrap_or(0)
    }

    fn conflicting_write(&self, candidate: &str) -> Option<(NodeId, String)> {
        for (active_node, paths) in &self.active_writes {
            for active_path in paths {
                if path_patterns_overlap(active_path, candidate) {
                    return Some((active_node.clone(), active_path.clone()));
                }
            }
        }
        None
    }
}

pub fn validate_worker_isolation(spec: &GraphSpec) -> Result<(), ResourceError> {
    let has_worker = spec.spec.nodes.iter().any(NodeSpec::is_model_worker);
    if has_worker && !spec.spec.execution.disable_nested_bandicot_agents {
        return Err(ResourceError::WorkerIsolationDisabled {
            flag: "disableNestedBandicotAgents",
        });
    }
    if has_worker && !spec.spec.execution.disable_provider_multi_agent_for_workers {
        return Err(ResourceError::WorkerIsolationDisabled {
            flag: "disableProviderMultiAgentForWorkers",
        });
    }
    for node in &spec.spec.nodes {
        for tool in &node.tool_allowlist {
            if node.is_model_worker() && is_nested_orchestration_tool(tool) {
                return Err(ResourceError::NestedOrchestrationTool {
                    node_id: node.id.clone(),
                    tool: tool.clone(),
                });
            }
        }
    }
    Ok(())
}

pub fn denied_worker_tools() -> BTreeSet<ToolName> {
    [
        "task",
        "spawn_agent",
        "agent",
        "subagent",
        "graph",
        "/graph",
        "swarm",
        "/swarm",
        "provider_multi_agent",
        "hosted_multi_agent",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn is_nested_orchestration_tool(tool: &str) -> bool {
    denied_worker_tools().contains(&tool.to_ascii_lowercase())
}

pub fn normalize_path_pattern(path: &str) -> String {
    let mut normalized = path.trim().replace('\\', "/");
    while let Some(rest) = normalized.strip_prefix("./") {
        normalized = rest.to_string();
    }
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized
        .trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_string()
}

pub fn path_patterns_overlap(left: &str, right: &str) -> bool {
    let left = normalize_path_pattern(left);
    let right = normalize_path_pattern(right);
    !left.is_empty()
        && !right.is_empty()
        && (left == right
            || left.starts_with(&format!("{right}/"))
            || right.starts_with(&format!("{left}/")))
}

fn normalized_write_set(node: &NodeSpec) -> Vec<String> {
    let mut paths = node
        .write_set
        .iter()
        .map(|path| normalize_path_pattern(path))
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}
