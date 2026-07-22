use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::scheduler::{AgentGraphScheduler, SchedulerConfig, SchedulerError};
use super::types::{
    AGENTGRAPH_API_VERSION, AGENTGRAPH_KIND, AGENTGRAPH_SCHEMA_VERSION, CachePolicy,
    CapabilityMode, EvidenceRequirement, ExecutionPolicy, FailurePolicy, GraphBudgets,
    GraphMetadata, GraphSpec, GraphSpecBody, IdempotencyPolicy, NetworkPolicy, NodeDefaults,
    NodeKind, NodeSpec, OrchestrationMode, RetryPolicy, ServiceTierPreference,
};
use super::validation::{ValidationError, ValidationOptions, validate_graph_spec};
use super::worker::FakeWorkerBackend;

pub const EXACT_100_WORKER_COUNT: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub total_worker_nodes: u32,
    pub configured_limit: u32,
    pub peak_active_workers: u32,
    pub queued_workers: u32,
    pub completed_workers: u32,
    pub failed_workers: u32,
    pub timed_out_workers: u32,
    pub cancelled_workers: u32,
    pub duration_ms: u128,
    pub fake_backend: bool,
}

#[derive(Debug, Error)]
pub enum BenchmarkError {
    #[error("graph policy must be swarm for the exact-100 benchmark")]
    NotSwarm,
    #[error("exact-100 validation failed: {0:?}")]
    Validation(Vec<ValidationError>),
    #[error("scheduler failed: {0}")]
    Scheduler(#[from] SchedulerError),
}

pub fn build_exact_100_worker_graph(name: &str) -> GraphSpec {
    let nodes = (0..EXACT_100_WORKER_COUNT)
        .map(|idx| independent_agent_node(format!("worker-{idx:03}")))
        .collect::<Vec<_>>();
    GraphSpec {
        api_version: AGENTGRAPH_API_VERSION.to_string(),
        kind: AGENTGRAPH_KIND.to_string(),
        metadata: GraphMetadata {
            name: name.to_string(),
            graph_version: AGENTGRAPH_SCHEMA_VERSION,
            labels: BTreeMap::new(),
        },
        spec: GraphSpecBody {
            objective: "Run exactly 100 independent workers to inventory repository modules, architecture boundaries, and implementation risks.".to_string(),
            execution: ExecutionPolicy {
                orchestration_policy: OrchestrationMode::Swarm,
                max_depth: 1,
                max_total_nodes: 100,
                max_active_model_calls: 100,
                disable_nested_bandicot_agents: true,
                disable_provider_multi_agent_for_workers: true,
            },
            budgets: GraphBudgets {
                max_wall_time_seconds: Some(3_600),
                max_input_tokens: Some(2_000_000),
                max_output_tokens: Some(500_000),
                max_model_calls: Some(100),
                max_node_attempts: Some(100),
                hard_stop: true,
                ..GraphBudgets::default()
            },
            defaults: NodeDefaults {
                model_selector: Some("worker-light".to_string()),
                capability_mode: CapabilityMode::ReadOnly,
                service_tier: ServiceTierPreference::Standard,
                max_input_tokens: Some(20_000),
                max_output_tokens: Some(5_000),
                max_tool_calls: Some(20),
                timeout_seconds: Some(600),
                ..NodeDefaults::default()
            },
            schemas: BTreeMap::from([(
                "worker-output".to_string(),
                serde_json::json!({
                    "type": "object",
                    "required": ["summary"],
                    "properties": { "summary": { "type": "string" } }
                }),
            )]),
            templates: BTreeSet::from([
                "overhead-role:root-coordinator".to_string(),
                "overhead-role:result-verifier".to_string(),
                "worker-role:repository-module-risk-inventory".to_string(),
            ]),
            model_selectors: BTreeSet::from(["worker-light".to_string(), "luna".to_string()]),
            credential_refs: BTreeSet::new(),
            resources: BTreeMap::new(),
            nodes,
            edges: Vec::new(),
        },
    }
}

pub fn validate_exact_100_worker_graph(spec: &GraphSpec) -> Result<(), BenchmarkError> {
    if spec.spec.execution.orchestration_policy != OrchestrationMode::Swarm {
        return Err(BenchmarkError::NotSwarm);
    }
    let report = validate_graph_spec(
        spec,
        &ValidationOptions {
            require_exact_ready_agents: Some(EXACT_100_WORKER_COUNT),
            max_total_nodes: Some(EXACT_100_WORKER_COUNT),
            ..ValidationOptions::default()
        },
    );
    if report.is_valid() {
        Ok(())
    } else {
        Err(BenchmarkError::Validation(report.errors))
    }
}

pub fn run_exact_100_fake_benchmark(
    max_active_model_calls: u32,
) -> Result<BenchmarkReport, BenchmarkError> {
    let started = Instant::now();
    let configured_limit = max_active_model_calls.max(1);
    let mut graph = build_exact_100_worker_graph("exact-100-fake-benchmark");
    graph.spec.execution.max_active_model_calls = configured_limit;
    validate_exact_100_worker_graph(&graph)?;

    let mut scheduler = AgentGraphScheduler::new(
        graph,
        SchedulerConfig {
            max_active_model_calls: configured_limit,
            plan_mode: false,
        },
    )?;
    let mut backend = FakeWorkerBackend::complete_immediately();
    let report = scheduler.run_to_completion(&mut backend, "exact-100-fake-run")?;
    let peak = report.peak_active_workers as u32;
    Ok(BenchmarkReport {
        total_worker_nodes: EXACT_100_WORKER_COUNT,
        configured_limit,
        peak_active_workers: peak,
        queued_workers: report.queued_workers as u32,
        completed_workers: report.completed_nodes as u32,
        failed_workers: report.failed_nodes as u32,
        timed_out_workers: report.timed_out_nodes as u32,
        cancelled_workers: report.cancelled_nodes as u32,
        duration_ms: started.elapsed().as_millis(),
        fake_backend: true,
    })
}

pub fn benchmark_store_path(root: &Path) -> std::path::PathBuf {
    root.join(".agent").join("agentgraph-benchmark.db")
}

fn independent_agent_node(id: String) -> NodeSpec {
    NodeSpec {
        id: id.clone(),
        kind: NodeKind::Agent,
        operation: None,
        objective: format!(
            "Inventory one repository module for architecture boundaries, ownership, risk, and verification needs as independent worker {id}."
        ),
        definition_of_done: vec![
            "return a NodeOutput with succeeded status".to_string(),
            "summarize module boundaries, dependencies, risks, and suggested verification"
                .to_string(),
        ],
        inputs: BTreeMap::new(),
        output_schema: None,
        output_schema_ref: Some("worker-output".to_string()),
        read_set: Vec::new(),
        write_set: Vec::new(),
        external_effects: Vec::new(),
        capability_mode: Some(CapabilityMode::ReadOnly),
        tool_allowlist: Vec::new(),
        tool_denylist: Vec::new(),
        network_policy: NetworkPolicy::default(),
        credential_refs: Vec::new(),
        model_selector: None,
        reasoning_effort: None,
        service_tier: Some(ServiceTierPreference::Standard),
        max_input_tokens: Some(20_000),
        max_output_tokens: Some(5_000),
        max_tool_calls: Some(20),
        timeout_seconds: Some(600),
        start_deadline_seconds: None,
        retry_policy: RetryPolicy::default(),
        idempotency_policy: IdempotencyPolicy::Idempotent,
        cache_policy: CachePolicy::Disabled,
        resource_claims: Vec::new(),
        evidence_requirements: vec![EvidenceRequirement {
            kind: "node-output".to_string(),
            required: true,
        }],
        verification_requirements: Vec::new(),
        failure_policy: FailurePolicy::FailGraph,
        compensation: None,
        estimated_duration_seconds: Some(1),
        tags: vec!["benchmark".to_string()],
        map: None,
        reduce: None,
        loop_policy: None,
        routes: Vec::new(),
    }
}
