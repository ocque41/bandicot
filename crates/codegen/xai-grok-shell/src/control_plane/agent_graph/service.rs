use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::ExecutionApproval;
use super::budget::BudgetPersistentState;
use super::normalization::canonical_graph_hash;
use super::store::{AgentGraphStore, GraphEvent, StoreError};
use super::topology::analyze_topology;
use super::types::{ArtifactRef, GraphSpec, NodeStatus, RunStatus};
use super::validation::{ValidationOptions, validate_graph_spec};

pub const GRAPH_ACP_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphValidationDto {
    pub schema_version: u32,
    pub valid: bool,
    pub normalized_spec_hash: Option<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPreviewDto {
    pub schema_version: u32,
    pub graph_id: String,
    pub graph_revision: u32,
    pub normalized_spec_hash: String,
    pub total_static_nodes: usize,
    pub maximum_dynamic_nodes: u64,
    pub initial_ready_width: u32,
    pub maximum_theoretical_width: u32,
    pub critical_path_length: u32,
    pub effective_concurrency: u32,
    pub budget_limits: super::types::GraphBudgets,
    pub estimated_model_calls: u64,
    pub estimated_input_tokens: EstimateRangeDto,
    pub estimated_output_tokens: EstimateRangeDto,
    pub estimated_cost_usd: Option<f64>,
    pub model_selectors: Vec<String>,
    pub model_selector_resolution: Vec<serde_json::Value>,
    pub service_tiers: Vec<super::types::ServiceTierPreference>,
    pub resource_pressure: BTreeMap<String, u64>,
    pub read_effect_count: usize,
    pub write_capable_nodes: usize,
    pub write_effect_count: usize,
    pub network_effect_count: usize,
    pub external_effect_count: usize,
    pub required_approval: bool,
    pub retry_policies: Vec<serde_json::Value>,
    pub join_policies: Vec<serde_json::Value>,
    pub loop_bounds: Vec<serde_json::Value>,
    pub validation_errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateRangeDto {
    pub minimum: u64,
    pub maximum: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStatusDto {
    pub schema_version: u32,
    pub graph_id: String,
    pub graph_revision: u32,
    pub run_id: String,
    pub normalized_spec_hash: String,
    pub status: RunStatus,
    pub total_nodes: usize,
    pub active_count: usize,
    pub ready_count: usize,
    pub retrying_count: usize,
    pub succeeded_count: usize,
    pub failed_count: usize,
    pub timed_out_count: usize,
    pub cancelled_count: usize,
    pub budget_state: BudgetPersistentState,
    pub rate_limit_state: Vec<serde_json::Value>,
    pub approval: Option<ExecutionApproval>,
    pub warnings: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub last_durable_event: Option<GraphEvent>,
}

pub struct AgentGraphService {
    repo_root: PathBuf,
    store_path: PathBuf,
}

impl AgentGraphService {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        let repo_root = repo_root.as_ref().to_path_buf();
        Self {
            store_path: repo_root.join(".agent/agentgraph.db"),
            repo_root,
        }
    }

    pub fn validate(&self, spec: &GraphSpec) -> GraphValidationDto {
        let report = validate_graph_spec(spec, &ValidationOptions::default());
        GraphValidationDto {
            schema_version: GRAPH_ACP_SCHEMA_VERSION,
            valid: report.is_valid(),
            normalized_spec_hash: report.normalized_hash,
            errors: report.errors.iter().map(ToString::to_string).collect(),
            warnings: report
                .warnings
                .iter()
                .map(|warning| format!("{}: {}", warning.code, warning.message))
                .collect(),
        }
    }

    pub fn preview(&self, spec: &GraphSpec) -> Result<GraphPreviewDto, String> {
        let validation = self.validate(spec);
        if !validation.valid {
            return Err(validation.errors.join("; "));
        }
        let topology = analyze_topology(spec).map_err(|error| error.to_string())?;
        let defaults = &spec.spec.defaults;
        let estimated_input_tokens = token_range(spec, |node| {
            node.max_input_tokens.or(defaults.max_input_tokens)
        });
        let estimated_output_tokens = token_range(spec, |node| {
            node.max_output_tokens.or(defaults.max_output_tokens)
        });
        let mut service_tiers = Vec::new();
        let mut resource_pressure = BTreeMap::new();
        for node in &spec.spec.nodes {
            let tier = node.effective_service_tier(defaults);
            if !service_tiers.contains(&tier) {
                service_tiers.push(tier);
            }
            for claim in &node.resource_claims {
                *resource_pressure.entry(claim.resource.clone()).or_default() +=
                    u64::from(claim.amount) * u64::from(node.expected_instance_count());
            }
        }
        Ok(GraphPreviewDto {
            schema_version: GRAPH_ACP_SCHEMA_VERSION,
            graph_id: spec.metadata.name.clone(),
            graph_revision: spec.metadata.graph_version,
            normalized_spec_hash: canonical_graph_hash(spec).map_err(|error| error.to_string())?,
            total_static_nodes: spec.spec.nodes.len(),
            maximum_dynamic_nodes: spec
                .spec
                .nodes
                .iter()
                .filter_map(|node| node.loop_policy.as_ref()?.max_generated_nodes)
                .map(u64::from)
                .sum(),
            initial_ready_width: topology.initial_ready_width,
            maximum_theoretical_width: topology.maximum_theoretical_width,
            critical_path_length: topology.critical_path_length,
            effective_concurrency: spec.spec.execution.max_active_model_calls.max(1),
            budget_limits: spec.spec.budgets.clone(),
            estimated_model_calls: spec
                .spec
                .nodes
                .iter()
                .filter(|node| node.is_model_worker())
                .map(|node| u64::from(node.expected_instance_count()))
                .sum(),
            estimated_input_tokens,
            estimated_output_tokens,
            // Pricing is provider- and account-dependent. A budget ceiling is
            // not an estimate, so preview reports Unknown until pricing exists.
            estimated_cost_usd: None,
            model_selectors: spec.spec.model_selectors.iter().cloned().collect(),
            model_selector_resolution: spec
                .spec
                .model_selectors
                .iter()
                .map(|selector| {
                    let candidates = super::model_selector::builtin_selector(selector)
                        .map(|resolved| {
                            resolved
                                .candidates
                                .into_iter()
                                .map(|candidate| candidate.model_slug)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    serde_json::json!({
                        "requestedSelector": selector,
                        "candidates": candidates,
                        "selectedCandidate": null,
                        "status": "live-catalog-resolution-at-dispatch"
                    })
                })
                .collect(),
            service_tiers,
            resource_pressure,
            read_effect_count: spec.spec.nodes.iter().map(|node| node.read_set.len()).sum(),
            write_capable_nodes: spec
                .spec
                .nodes
                .iter()
                .filter(|node| {
                    node.effective_capability(defaults) > super::types::CapabilityMode::ReadOnly
                })
                .count(),
            write_effect_count: spec
                .spec
                .nodes
                .iter()
                .map(|node| node.write_set.len())
                .sum(),
            network_effect_count: spec
                .spec
                .nodes
                .iter()
                .filter(|node| {
                    node.network_policy.mode != super::types::NetworkMode::Denied
                        || node
                            .external_effects
                            .contains(&super::types::ExternalEffect::Network)
                })
                .count(),
            external_effect_count: spec
                .spec
                .nodes
                .iter()
                .map(|node| node.external_effects.len())
                .sum(),
            required_approval: super::approval::graph_requires_execution_approval(spec),
            retry_policies: spec
                .spec
                .nodes
                .iter()
                .filter(|node| node.retry_policy.max_attempts > 1)
                .map(|node| serde_json::json!({ "nodeId": node.id, "policy": node.retry_policy }))
                .collect(),
            join_policies: spec
                .spec
                .edges
                .iter()
                .filter_map(|edge| {
                    edge.join.as_ref().map(|join| {
                        serde_json::json!({
                            "edgeId": edge.stable_id(),
                            "from": edge.from,
                            "to": edge.to,
                            "join": join,
                        })
                    })
                })
                .collect(),
            loop_bounds: spec
                .spec
                .nodes
                .iter()
                .filter_map(|node| {
                    node.loop_policy
                        .as_ref()
                        .map(|bounds| serde_json::json!({ "nodeId": node.id, "bounds": bounds }))
                })
                .collect(),
            validation_errors: validation.errors,
            warnings: validation.warnings,
        })
    }

    pub fn draft(
        &self,
        spec: &GraphSpec,
        session_id: Option<&str>,
    ) -> Result<GraphStatusDto, String> {
        let validation = self.validate(spec);
        if !validation.valid {
            return Err(validation.errors.join("; "));
        }
        let mut store =
            AgentGraphStore::open(&self.store_path).map_err(|error| error.to_string())?;
        let run_id = store
            .create_run(spec, session_id, &self.repo_root)
            .map_err(|error| error.to_string())?;
        if let Some(session_id) = session_id {
            store
                .attach_active_run(session_id, &self.repo_root, &run_id)
                .map_err(|error| error.to_string())?;
        }
        self.status(&run_id).map_err(|error| error.to_string())
    }

    pub fn status(&self, run_id: &str) -> Result<GraphStatusDto, StoreError> {
        let store = AgentGraphStore::open(&self.store_path)?;
        let spec = store.graph_spec_for_run(run_id)?;
        let snapshot = store.replay_run(run_id)?;
        let count = |status: NodeStatus| {
            snapshot
                .node_states
                .iter()
                .filter(|node| node.status == status)
                .count()
        };
        let artifacts = snapshot
            .node_states
            .iter()
            .filter_map(|node| node.output.as_ref())
            .flat_map(|output| output.artifacts.clone())
            .collect();
        Ok(GraphStatusDto {
            schema_version: GRAPH_ACP_SCHEMA_VERSION,
            graph_id: spec.metadata.name.clone(),
            graph_revision: spec.metadata.graph_version,
            run_id: run_id.to_string(),
            normalized_spec_hash: canonical_graph_hash(&spec)?,
            status: snapshot.status,
            total_nodes: snapshot.node_states.len(),
            active_count: count(NodeStatus::Leased)
                + count(NodeStatus::Starting)
                + count(NodeStatus::Running)
                + count(NodeStatus::Compensating),
            ready_count: count(NodeStatus::Ready) + count(NodeStatus::Pending),
            retrying_count: count(NodeStatus::Retrying),
            succeeded_count: count(NodeStatus::Succeeded),
            failed_count: count(NodeStatus::Failed),
            timed_out_count: count(NodeStatus::TimedOut),
            cancelled_count: count(NodeStatus::Cancelled),
            budget_state: store.budget_state(run_id)?,
            rate_limit_state: super::rate_limit::observed_capacity_snapshot()
                .into_iter()
                .filter_map(|observation| serde_json::to_value(observation).ok())
                .collect(),
            approval: store.execution_approval(run_id)?,
            warnings: self.validate(&spec).warnings,
            artifacts,
            last_durable_event: snapshot.events.last().cloned(),
        })
    }

    pub fn transition(
        &self,
        run_id: &str,
        status: RunStatus,
    ) -> Result<GraphStatusDto, StoreError> {
        let mut store = AgentGraphStore::open(&self.store_path)?;
        store.mark_run_status(run_id, status)?;
        self.status(run_id)
    }

    pub fn retry_failed(&self, run_id: &str) -> Result<GraphStatusDto, StoreError> {
        let mut store = AgentGraphStore::open(&self.store_path)?;
        let snapshot = store.replay_run(run_id)?;
        for node in snapshot.node_states.iter().filter(|node| {
            matches!(
                node.status,
                NodeStatus::Failed | NodeStatus::TimedOut | NodeStatus::Stale
            )
        }) {
            store.mark_node_status(run_id, &node.node_id, NodeStatus::Ready)?;
        }
        store.mark_run_status(run_id, RunStatus::Running)?;
        self.status(run_id)
    }

    pub fn export(&self, run_id: &str) -> Result<serde_json::Value, StoreError> {
        let store = AgentGraphStore::open(&self.store_path)?;
        Ok(serde_json::json!({
            "schemaVersion": GRAPH_ACP_SCHEMA_VERSION,
            "spec": store.graph_spec_for_run(run_id)?,
            "run": store.replay_run(run_id)?,
            "budget": store.budget_state(run_id)?,
            "approval": store.execution_approval(run_id)?,
        }))
    }

    pub fn cleanup(&self, run_id: &str) -> Result<(), StoreError> {
        let mut store = AgentGraphStore::open(&self.store_path)?;
        store.cleanup_run(run_id)
    }
}

fn token_range(
    spec: &GraphSpec,
    maximum_for_node: impl Fn(&super::types::NodeSpec) -> Option<u64>,
) -> EstimateRangeDto {
    let mut maximum = Some(0_u64);
    for node in spec.spec.nodes.iter().filter(|node| node.is_model_worker()) {
        maximum = maximum.zip(maximum_for_node(node)).map(|(sum, value)| {
            sum.saturating_add(value.saturating_mul(u64::from(node.expected_instance_count())))
        });
    }
    EstimateRangeDto {
        minimum: 0,
        maximum,
    }
}
