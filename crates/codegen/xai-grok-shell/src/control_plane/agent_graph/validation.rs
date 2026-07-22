use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as JsonValue;
use thiserror::Error;

use super::normalization::{NormalizedGraphSpec, canonical_graph_hash, normalize_graph_spec};
use super::predicate::{validate_json_path, validate_predicate};
use super::topology::{TopologyError, TopologyReport, analyze_topology};
use super::types::{
    AGENTGRAPH_API_VERSION, AGENTGRAPH_KIND, AGENTGRAPH_SCHEMA_VERSION, CachePolicy,
    CapabilityMode, EdgeKind, GraphSpec, IdempotencyPolicy, JoinPolicy, JoinSpec, NodeId, NodeKind,
    NodeSpec, OrchestrationMode, ResourceKind, ToolName,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOptions {
    pub parent_capability_ceiling: CapabilityMode,
    pub available_tools: Option<BTreeSet<ToolName>>,
    pub parent_tool_allowlist: Option<BTreeSet<ToolName>>,
    pub allowed_roots: Vec<String>,
    pub plan_mode: bool,
    pub require_exact_ready_agents: Option<u32>,
    pub max_total_nodes: Option<u32>,
    pub max_dynamic_nodes: Option<u32>,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            parent_capability_ceiling: CapabilityMode::ExternalEffect,
            available_tools: None,
            parent_tool_allowlist: None,
            allowed_roots: Vec::new(),
            plan_mode: false,
            require_exact_ready_agents: None,
            max_total_nodes: None,
            max_dynamic_nodes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
    pub normalized_hash: Option<String>,
    pub topology: Option<TopologyReport>,
    pub exact_ready: Option<ExactReadyReport>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactReadyReport {
    pub required_ready_agents: u32,
    pub actual_ready_agents: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationWarning {
    pub code: &'static str,
    pub message: String,
    pub node_id: Option<NodeId>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("unsupported apiVersion `{found}`")]
    UnsupportedApiVersion { found: String },
    #[error("unsupported kind `{found}`")]
    UnsupportedKind { found: String },
    #[error("unsupported graph schema version `{found}`")]
    UnsupportedSchemaVersion { found: u32 },
    #[error("invalid id `{id}` for {field}: {reason}")]
    InvalidId {
        field: &'static str,
        id: String,
        reason: String,
    },
    #[error("duplicate node id `{node_id}`")]
    DuplicateNode { node_id: NodeId },
    #[error("duplicate edge id `{edge_id}`")]
    DuplicateEdge { edge_id: String },
    #[error("node `{node_id}` references missing node `{referenced}`")]
    MissingNodeReference { node_id: NodeId, referenced: NodeId },
    #[error("edge `{edge_id}` references missing node `{referenced}`")]
    MissingEdgeNodeReference { edge_id: String, referenced: NodeId },
    #[error("node `{node_id}` input `{input}` has no producer `{producer}`")]
    MissingInputProducer {
        node_id: NodeId,
        input: String,
        producer: NodeId,
    },
    #[error("node `{node_id}` references missing schema `{schema_ref}`")]
    MissingSchemaReference { node_id: NodeId, schema_ref: String },
    #[error("node `{node_id}` references missing template `{template_ref}`")]
    MissingTemplateReference {
        node_id: NodeId,
        template_ref: String,
    },
    #[error("node `{node_id}` references missing model selector `{model_selector}`")]
    MissingModelSelector {
        node_id: NodeId,
        model_selector: String,
    },
    #[error("node `{node_id}` references missing credential `{credential_ref}`")]
    MissingCredentialReference {
        node_id: NodeId,
        credential_ref: String,
    },
    #[error("node `{node_id}` references missing resource `{resource}`")]
    MissingResourceReference { node_id: NodeId, resource: String },
    #[error("data edge `{edge_id}` has no data binding")]
    DataEdgeWithoutBinding { edge_id: String },
    #[error("graph contains a general cycle involving {nodes:?}")]
    Cycle { nodes: Vec<NodeId> },
    #[error("node `{node_id}` has an invalid predicate: {reason}")]
    InvalidPredicate { node_id: NodeId, reason: String },
    #[error("node `{node_id}` has an invalid json path `{path}`: {reason}")]
    InvalidJsonPath {
        node_id: NodeId,
        path: String,
        reason: String,
    },
    #[error("loop node `{node_id}` is missing bound `{bound}`")]
    UnboundedLoop {
        node_id: NodeId,
        bound: &'static str,
    },
    #[error("map node `{node_id}` is missing a dynamic expansion bound")]
    UnboundedMap { node_id: NodeId },
    #[error("map node `{node_id}` does not declare deterministic instance ids")]
    NonDeterministicMapIds { node_id: NodeId },
    #[error("join on edge `{edge_id}` is invalid: {reason}")]
    InvalidJoin { edge_id: String, reason: String },
    #[error("retry policy on node `{node_id}` is not bounded")]
    UnboundedRetry { node_id: NodeId },
    #[error("non-idempotent retry on node `{node_id}` requires compensation")]
    NonIdempotentRetryNeedsCompensation { node_id: NodeId },
    #[error("node `{node_id}` capability `{capability:?}` exceeds parent ceiling `{ceiling:?}`")]
    CapabilityWidening {
        node_id: NodeId,
        capability: CapabilityMode,
        ceiling: CapabilityMode,
    },
    #[error("plan mode rejects write-capable node `{node_id}`")]
    PlanModeWriter { node_id: NodeId },
    #[error("node `{node_id}` tool `{tool}` is unavailable")]
    UnavailableTool { node_id: NodeId, tool: String },
    #[error("node `{node_id}` tool `{tool}` widens parent authority")]
    ToolWidening { node_id: NodeId, tool: String },
    #[error("worker node `{node_id}` cannot receive nested orchestration tool `{tool}`")]
    NestedOrchestrationTool { node_id: NodeId, tool: String },
    #[error("worker isolation flag `{flag}` must be true")]
    WorkerIsolationDisabled { flag: &'static str },
    #[error("node `{node_id}` path `{path}` is invalid: {reason}")]
    InvalidPath {
        node_id: NodeId,
        path: String,
        reason: String,
    },
    #[error("write set conflict between `{left}` and `{right}` on `{path}`")]
    WriteSetConflict {
        left: NodeId,
        right: NodeId,
        path: String,
    },
    #[error("node `{node_id}` resource `{resource}` claim `{amount}` exceeds limit `{limit}`")]
    ResourceLimitExceeded {
        node_id: NodeId,
        resource: String,
        amount: u32,
        limit: u32,
    },
    #[error("budget `{field}` must be greater than zero")]
    InvalidBudget { field: &'static str },
    #[error("graph dynamic expansion `{actual}` exceeds maximum `{limit}`")]
    DynamicExpansionTooLarge { actual: u32, limit: u32 },
    #[error("graph node count `{actual}` exceeds maximum `{limit}`")]
    GraphTooLarge { actual: u32, limit: u32 },
    #[error("exact-ready benchmark expected `{required}` ready agents but graph has `{actual}`")]
    ExactReadyMismatch { required: u32, actual: u32 },
    #[error("normalization failed: {reason}")]
    NormalizationFailed { reason: String },
}

pub fn validate_graph_spec(spec: &GraphSpec, options: &ValidationOptions) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    validate_header(spec, &mut errors);
    let node_index = validate_node_ids(spec, &mut errors);
    validate_edge_ids(spec, &mut errors);
    validate_graph_limits(spec, options, &mut errors);
    validate_budgets(spec, &mut errors);
    validate_execution_policy(spec, &node_index, &mut errors);
    validate_nodes(spec, options, &node_index, &mut errors, &mut warnings);
    validate_edges(spec, &node_index, &mut errors);
    validate_write_conflicts(spec, &mut errors);

    let topology = match analyze_topology(spec) {
        Ok(report) => {
            validate_exact_ready(&report, options, &mut errors);
            Some(report)
        }
        Err(TopologyError::Cycle { nodes }) => {
            errors.push(ValidationError::Cycle { nodes });
            None
        }
        Err(TopologyError::DuplicateNode { node_id }) => {
            errors.push(ValidationError::DuplicateNode { node_id });
            None
        }
        Err(TopologyError::UnknownNode { node_id }) => {
            errors.push(ValidationError::MissingEdgeNodeReference {
                edge_id: "<implicit>".to_string(),
                referenced: node_id,
            });
            None
        }
    };

    GraphLinter::default().lint(spec, topology.as_ref(), &mut warnings);

    let normalized_hash = match canonical_graph_hash(spec) {
        Ok(hash) => Some(hash),
        Err(err) => {
            errors.push(ValidationError::NormalizationFailed {
                reason: err.to_string(),
            });
            None
        }
    };

    let exact_ready = options
        .require_exact_ready_agents
        .map(|required| ExactReadyReport {
            required_ready_agents: required,
            actual_ready_agents: topology
                .as_ref()
                .map(|report| report.initial_ready_agent_width)
                .unwrap_or(0),
        });

    ValidationReport {
        errors,
        warnings,
        normalized_hash,
        topology,
        exact_ready,
    }
}

#[derive(Debug, Clone)]
pub struct CompiledGraph {
    pub normalized: NormalizedGraphSpec,
    pub topology: TopologyReport,
    pub warnings: Vec<ValidationWarning>,
}

#[derive(Debug, Clone, Default)]
pub struct GraphCompiler {
    options: ValidationOptions,
}

impl GraphCompiler {
    pub fn new(options: ValidationOptions) -> Self {
        Self { options }
    }

    pub fn compile(&self, spec: &GraphSpec) -> Result<CompiledGraph, ValidationReport> {
        let report = validate_graph_spec(spec, &self.options);
        if !report.is_valid() {
            return Err(report);
        }
        let normalized = match normalize_graph_spec(spec) {
            Ok(normalized) => normalized,
            Err(err) => {
                let mut failed = report;
                failed.errors.push(ValidationError::NormalizationFailed {
                    reason: err.to_string(),
                });
                return Err(failed);
            }
        };
        Ok(CompiledGraph {
            normalized,
            topology: report
                .topology
                .clone()
                .expect("valid graph must have topology"),
            warnings: report.warnings,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct GraphLinter;

impl GraphLinter {
    pub fn lint(
        &self,
        spec: &GraphSpec,
        topology: Option<&TopologyReport>,
        warnings: &mut Vec<ValidationWarning>,
    ) {
        for node in &spec.spec.nodes {
            if node.is_model_worker()
                && node.output_schema.is_none()
                && node.output_schema_ref.is_none()
            {
                warnings.push(ValidationWarning {
                    code: "weak-output-schema",
                    message: "model worker should declare an output schema".to_string(),
                    node_id: Some(node.id.clone()),
                });
            }
            if node.is_model_worker() && node.evidence_requirements.is_empty() {
                warnings.push(ValidationWarning {
                    code: "missing-evidence-requirements",
                    message: "model worker should declare evidence requirements".to_string(),
                    node_id: Some(node.id.clone()),
                });
            }
            if node.objective.split_whitespace().count() <= 3 && node.is_model_worker() {
                warnings.push(ValidationWarning {
                    code: "small-agent-node",
                    message: "agent node objective is very small".to_string(),
                    node_id: Some(node.id.clone()),
                });
            }
            if node.kind == NodeKind::Barrier {
                let incoming = spec
                    .spec
                    .edges
                    .iter()
                    .filter(|edge| edge.to == node.id)
                    .count();
                let outgoing = spec
                    .spec
                    .edges
                    .iter()
                    .filter(|edge| edge.from == node.id)
                    .count();
                if incoming <= 1 || outgoing <= 1 {
                    warnings.push(ValidationWarning {
                        code: "unnecessary-barrier",
                        message: "barrier has one or fewer inputs or outputs and may be removable"
                            .to_string(),
                        node_id: Some(node.id.clone()),
                    });
                }
            }
        }

        for edge in &spec.spec.edges {
            if edge.kind == EdgeKind::Control
                && edge.bindings.is_empty()
                && edge.condition.is_none()
                && edge.join.is_none()
            {
                warnings.push(ValidationWarning {
                    code: "fake-dependency-candidate",
                    message: format!(
                        "control-only dependency {} -> {} carries no data, condition, or join policy",
                        edge.from, edge.to
                    ),
                    node_id: Some(edge.to.clone()),
                });
            }
        }

        if let Some(topology) = topology {
            if topology.critical_path_length > 0
                && topology.maximum_theoretical_width == 1
                && topology.node_count > 10
            {
                warnings.push(ValidationWarning {
                    code: "single-bottleneck-critical-path",
                    message: "graph has a long single-lane critical path".to_string(),
                    node_id: None,
                });
            }
        }
    }
}

fn validate_header(spec: &GraphSpec, errors: &mut Vec<ValidationError>) {
    if spec.api_version != AGENTGRAPH_API_VERSION {
        errors.push(ValidationError::UnsupportedApiVersion {
            found: spec.api_version.clone(),
        });
    }
    if spec.kind != AGENTGRAPH_KIND {
        errors.push(ValidationError::UnsupportedKind {
            found: spec.kind.clone(),
        });
    }
    if spec.schema_version() != AGENTGRAPH_SCHEMA_VERSION {
        errors.push(ValidationError::UnsupportedSchemaVersion {
            found: spec.schema_version(),
        });
    }
    validate_id("metadata.name", &spec.metadata.name, errors);
}

fn validate_node_ids<'a>(
    spec: &'a GraphSpec,
    errors: &mut Vec<ValidationError>,
) -> BTreeMap<NodeId, &'a NodeSpec> {
    let mut node_index = BTreeMap::new();
    for node in &spec.spec.nodes {
        validate_id("node.id", &node.id, errors);
        if node_index.insert(node.id.clone(), node).is_some() {
            errors.push(ValidationError::DuplicateNode {
                node_id: node.id.clone(),
            });
        }
    }
    node_index
}

fn validate_edge_ids(spec: &GraphSpec, errors: &mut Vec<ValidationError>) {
    let mut ids = BTreeSet::new();
    for edge in &spec.spec.edges {
        let edge_id = edge.stable_id();
        if let Some(id) = &edge.id {
            validate_id("edge.id", id, errors);
        }
        if !ids.insert(edge_id.clone()) {
            errors.push(ValidationError::DuplicateEdge { edge_id });
        }
    }
}

fn validate_graph_limits(
    spec: &GraphSpec,
    options: &ValidationOptions,
    errors: &mut Vec<ValidationError>,
) {
    let node_count = spec.spec.nodes.len() as u32;
    let spec_limit = spec.spec.execution.max_total_nodes;
    if node_count > spec_limit {
        errors.push(ValidationError::GraphTooLarge {
            actual: node_count,
            limit: spec_limit,
        });
    }
    if let Some(limit) = options.max_total_nodes {
        if node_count > limit {
            errors.push(ValidationError::GraphTooLarge {
                actual: node_count,
                limit,
            });
        }
    }

    let dynamic_nodes = spec
        .spec
        .nodes
        .iter()
        .filter_map(|node| node.map.as_ref())
        .filter_map(|map| map.expected_instances.or(map.max_generated_instances))
        .sum::<u32>();
    if let Some(limit) = options.max_dynamic_nodes {
        if dynamic_nodes > limit {
            errors.push(ValidationError::DynamicExpansionTooLarge {
                actual: dynamic_nodes,
                limit,
            });
        }
    }
}

fn validate_budgets(spec: &GraphSpec, errors: &mut Vec<ValidationError>) {
    let budgets = &spec.spec.budgets;
    if budgets.max_wall_time_seconds == Some(0) {
        errors.push(ValidationError::InvalidBudget {
            field: "maxWallTimeSeconds",
        });
    }
    if budgets.max_input_tokens == Some(0) {
        errors.push(ValidationError::InvalidBudget {
            field: "maxInputTokens",
        });
    }
    if budgets.max_output_tokens == Some(0) {
        errors.push(ValidationError::InvalidBudget {
            field: "maxOutputTokens",
        });
    }
    if budgets
        .max_estimated_cost_usd
        .is_some_and(|cost| cost <= 0.0)
    {
        errors.push(ValidationError::InvalidBudget {
            field: "maxEstimatedCostUsd",
        });
    }
}

fn validate_execution_policy(
    spec: &GraphSpec,
    node_index: &BTreeMap<NodeId, &NodeSpec>,
    errors: &mut Vec<ValidationError>,
) {
    let has_worker = node_index.values().any(|node| node.is_model_worker());
    if spec.spec.execution.max_depth > 1 {
        errors.push(ValidationError::WorkerIsolationDisabled {
            flag: "maxDepth<=1",
        });
    }
    if has_worker && !spec.spec.execution.disable_nested_bandicot_agents {
        errors.push(ValidationError::WorkerIsolationDisabled {
            flag: "disableNestedBandicotAgents",
        });
    }
    if has_worker && !spec.spec.execution.disable_provider_multi_agent_for_workers {
        errors.push(ValidationError::WorkerIsolationDisabled {
            flag: "disableProviderMultiAgentForWorkers",
        });
    }
    if spec.spec.execution.orchestration_policy == OrchestrationMode::Swarm
        && spec.spec.execution.max_active_model_calls > 100
    {
        errors.push(ValidationError::InvalidBudget {
            field: "maxActiveModelCalls",
        });
    }
}

fn validate_nodes(
    spec: &GraphSpec,
    options: &ValidationOptions,
    node_index: &BTreeMap<NodeId, &NodeSpec>,
    errors: &mut Vec<ValidationError>,
    warnings: &mut Vec<ValidationWarning>,
) {
    let known_nodes = node_index.keys().cloned().collect::<BTreeSet<_>>();
    for node in &spec.spec.nodes {
        validate_node_references(spec, node, node_index, errors);
        validate_node_predicates(node, &known_nodes, errors);
        validate_loop(node, &known_nodes, errors);
        validate_map(node, spec, node_index, errors);
        validate_retry(node, errors);
        validate_authority(spec, options, node, errors);
        validate_paths(node, options, errors);
        validate_resources(spec, node, errors);
        validate_node_contract(node, warnings);
    }
}

fn validate_node_references(
    spec: &GraphSpec,
    node: &NodeSpec,
    node_index: &BTreeMap<NodeId, &NodeSpec>,
    errors: &mut Vec<ValidationError>,
) {
    for (input, binding) in &node.inputs {
        if !node_index.contains_key(&binding.from_node) {
            errors.push(ValidationError::MissingInputProducer {
                node_id: node.id.clone(),
                input: input.clone(),
                producer: binding.from_node.clone(),
            });
        }
        if let Some(schema_ref) = &binding.schema_ref {
            validate_schema_ref(spec, node, schema_ref, errors);
        }
        if let Err(err) = validate_json_path(&binding.path) {
            errors.push(ValidationError::InvalidJsonPath {
                node_id: node.id.clone(),
                path: binding.path.clone(),
                reason: err.to_string(),
            });
        }
    }
    if let Some(schema_ref) = &node.output_schema_ref {
        validate_schema_ref(spec, node, schema_ref, errors);
    }
    if let Some(compensation) = &node.compensation {
        if !node_index.contains_key(compensation) {
            errors.push(ValidationError::MissingNodeReference {
                node_id: node.id.clone(),
                referenced: compensation.clone(),
            });
        }
    }
    if let Some(model_selector) = &node.model_selector {
        if !spec.spec.model_selectors.is_empty()
            && !spec.spec.model_selectors.contains(model_selector)
        {
            errors.push(ValidationError::MissingModelSelector {
                node_id: node.id.clone(),
                model_selector: model_selector.clone(),
            });
        }
    }
    for credential_ref in &node.credential_refs {
        if !spec.spec.credential_refs.contains(credential_ref) {
            errors.push(ValidationError::MissingCredentialReference {
                node_id: node.id.clone(),
                credential_ref: credential_ref.clone(),
            });
        }
    }
    if let Some(template) = node
        .operation
        .as_ref()
        .filter(|operation| operation.starts_with("bundled://"))
    {
        if !spec.spec.templates.is_empty() && !spec.spec.templates.contains(template) {
            errors.push(ValidationError::MissingTemplateReference {
                node_id: node.id.clone(),
                template_ref: template.clone(),
            });
        }
    }
}

fn validate_schema_ref(
    spec: &GraphSpec,
    node: &NodeSpec,
    schema_ref: &str,
    errors: &mut Vec<ValidationError>,
) {
    if !spec.spec.schemas.contains_key(schema_ref) {
        errors.push(ValidationError::MissingSchemaReference {
            node_id: node.id.clone(),
            schema_ref: schema_ref.to_string(),
        });
    }
}

fn validate_node_predicates(
    node: &NodeSpec,
    known_nodes: &BTreeSet<NodeId>,
    errors: &mut Vec<ValidationError>,
) {
    let mut has_fallback = false;
    for route in &node.routes {
        has_fallback |= route.fallback;
        if let Err(err) = validate_predicate(&route.predicate, known_nodes) {
            errors.push(ValidationError::InvalidPredicate {
                node_id: node.id.clone(),
                reason: err.to_string(),
            });
        }
    }
    if node.kind == NodeKind::Router && !node.routes.is_empty() && !has_fallback {
        errors.push(ValidationError::InvalidPredicate {
            node_id: node.id.clone(),
            reason: "router routes must include an explicit fallback".to_string(),
        });
    }
}

fn validate_loop(
    node: &NodeSpec,
    known_nodes: &BTreeSet<NodeId>,
    errors: &mut Vec<ValidationError>,
) {
    if node.kind != NodeKind::Loop {
        return;
    }

    let Some(loop_policy) = &node.loop_policy else {
        for bound in LOOP_BOUNDS {
            errors.push(ValidationError::UnboundedLoop {
                node_id: node.id.clone(),
                bound,
            });
        }
        return;
    };

    if loop_policy.max_iterations.is_none_or(|value| value == 0) {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "maxIterations",
        });
    }
    if loop_policy
        .max_generated_nodes
        .is_none_or(|value| value == 0)
    {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "maxGeneratedNodes",
        });
    }
    if loop_policy.max_input_tokens.is_none_or(|value| value == 0) {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "maxInputTokens",
        });
    }
    if loop_policy.max_output_tokens.is_none_or(|value| value == 0) {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "maxOutputTokens",
        });
    }
    if loop_policy
        .max_wall_time_seconds
        .is_none_or(|value| value == 0)
    {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "maxWallTimeSeconds",
        });
    }
    if loop_policy
        .progress_metric
        .as_deref()
        .unwrap_or("")
        .is_empty()
    {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "progressMetric",
        });
    }
    if loop_policy.no_progress_limit.is_none_or(|value| value == 0) {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "noProgressLimit",
        });
    }
    if loop_policy
        .deduplication_key
        .as_deref()
        .unwrap_or("")
        .is_empty()
    {
        errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "deduplicationKey",
        });
    }
    match &loop_policy.terminal_predicate {
        Some(predicate) => {
            if let Err(err) = validate_predicate(predicate, known_nodes) {
                errors.push(ValidationError::InvalidPredicate {
                    node_id: node.id.clone(),
                    reason: err.to_string(),
                });
            }
        }
        None => errors.push(ValidationError::UnboundedLoop {
            node_id: node.id.clone(),
            bound: "terminalPredicate",
        }),
    }
}

const LOOP_BOUNDS: [&str; 9] = [
    "maxIterations",
    "maxGeneratedNodes",
    "maxInputTokens",
    "maxOutputTokens",
    "maxWallTimeSeconds",
    "progressMetric",
    "noProgressLimit",
    "deduplicationKey",
    "terminalPredicate",
];

fn validate_map(
    node: &NodeSpec,
    spec: &GraphSpec,
    node_index: &BTreeMap<NodeId, &NodeSpec>,
    errors: &mut Vec<ValidationError>,
) {
    if node.kind != NodeKind::MapAgent {
        return;
    }
    let Some(map) = &node.map else {
        errors.push(ValidationError::UnboundedMap {
            node_id: node.id.clone(),
        });
        return;
    };
    if !node_index.contains_key(&map.from_node) {
        errors.push(ValidationError::MissingNodeReference {
            node_id: node.id.clone(),
            referenced: map.from_node.clone(),
        });
    }
    if let Err(err) = validate_json_path(&map.path) {
        errors.push(ValidationError::InvalidJsonPath {
            node_id: node.id.clone(),
            path: map.path.clone(),
            reason: err.to_string(),
        });
    }
    if map
        .expected_instances
        .or(map.max_generated_instances)
        .is_none_or(|bound| bound == 0)
    {
        errors.push(ValidationError::UnboundedMap {
            node_id: node.id.clone(),
        });
    }
    if !map.deterministic_instance_ids {
        errors.push(ValidationError::NonDeterministicMapIds {
            node_id: node.id.clone(),
        });
    }
    if let Some(schema_ref) = &map.item_schema_ref {
        validate_schema_ref(spec, node, schema_ref, errors);
    } else {
        errors.push(ValidationError::MissingSchemaReference {
            node_id: node.id.clone(),
            schema_ref: "<map.itemSchemaRef>".to_string(),
        });
    }
    if let Some(schema_ref) = &map.output_schema_ref {
        validate_schema_ref(spec, node, schema_ref, errors);
    }
}

fn validate_retry(node: &NodeSpec, errors: &mut Vec<ValidationError>) {
    if node.retry_policy.max_attempts == 0 || node.retry_policy.max_attempts > 10 {
        errors.push(ValidationError::UnboundedRetry {
            node_id: node.id.clone(),
        });
    }
    let has_side_effect = !node.external_effects.is_empty()
        || !node.write_set.is_empty()
        || node.idempotency_policy == IdempotencyPolicy::NonIdempotent;
    if has_side_effect
        && node.idempotency_policy == IdempotencyPolicy::NonIdempotent
        && node.retry_policy.max_attempts > 1
        && node.compensation.is_none()
        && node.failure_policy != super::types::FailurePolicy::Compensate
    {
        errors.push(ValidationError::NonIdempotentRetryNeedsCompensation {
            node_id: node.id.clone(),
        });
    }
}

fn validate_authority(
    spec: &GraphSpec,
    options: &ValidationOptions,
    node: &NodeSpec,
    errors: &mut Vec<ValidationError>,
) {
    let capability = node.effective_capability(&spec.spec.defaults);
    if capability > options.parent_capability_ceiling {
        errors.push(ValidationError::CapabilityWidening {
            node_id: node.id.clone(),
            capability,
            ceiling: options.parent_capability_ceiling,
        });
    }
    if options.plan_mode && capability > CapabilityMode::ReadOnly {
        errors.push(ValidationError::PlanModeWriter {
            node_id: node.id.clone(),
        });
    }

    for tool in &node.tool_allowlist {
        if let Some(available) = &options.available_tools {
            if !available.contains(tool) {
                errors.push(ValidationError::UnavailableTool {
                    node_id: node.id.clone(),
                    tool: tool.clone(),
                });
            }
        }
        if let Some(parent_allowlist) = &options.parent_tool_allowlist {
            if !parent_allowlist.contains(tool) {
                errors.push(ValidationError::ToolWidening {
                    node_id: node.id.clone(),
                    tool: tool.clone(),
                });
            }
        }
        if node.is_model_worker() && is_nested_orchestration_tool(tool) {
            errors.push(ValidationError::NestedOrchestrationTool {
                node_id: node.id.clone(),
                tool: tool.clone(),
            });
        }
    }

    if node.network_policy.mode == super::types::NetworkMode::Unrestricted
        && capability < CapabilityMode::ExternalEffect
    {
        errors.push(ValidationError::CapabilityWidening {
            node_id: node.id.clone(),
            capability: CapabilityMode::ExternalEffect,
            ceiling: capability,
        });
    }
}

fn is_nested_orchestration_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "task"
            | "spawn_agent"
            | "agent"
            | "subagent"
            | "graph"
            | "/graph"
            | "swarm"
            | "/swarm"
            | "provider_multi_agent"
            | "hosted_multi_agent"
    )
}

fn validate_paths(node: &NodeSpec, options: &ValidationOptions, errors: &mut Vec<ValidationError>) {
    for path in node.read_set.iter().chain(node.write_set.iter()) {
        if let Err(reason) = validate_path_pattern(path, &options.allowed_roots) {
            errors.push(ValidationError::InvalidPath {
                node_id: node.id.clone(),
                path: path.clone(),
                reason,
            });
        }
    }
}

fn validate_path_pattern(path: &str, allowed_roots: &[String]) -> Result<(), String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    if normalized.contains('\0') {
        return Err("path contains a NUL byte".to_string());
    }
    if normalized.starts_with('/') || normalized.starts_with("~/") || normalized == "~" {
        return Err("path must be repository-relative".to_string());
    }
    if normalized.contains("://") || normalized.starts_with('$') {
        return Err("path must not be a URL or environment expansion".to_string());
    }
    for segment in normalized.split('/') {
        if segment == ".." {
            return Err("path traversal is not allowed".to_string());
        }
    }
    if !allowed_roots.is_empty() {
        let without_dot = normalized.trim_start_matches("./");
        let allowed = allowed_roots.iter().any(|root| {
            let root = root.trim_matches('/');
            without_dot == root || without_dot.starts_with(&format!("{root}/"))
        });
        if !allowed {
            return Err("path is outside allowed roots".to_string());
        }
    }
    Ok(())
}

fn validate_resources(spec: &GraphSpec, node: &NodeSpec, errors: &mut Vec<ValidationError>) {
    for claim in &node.resource_claims {
        let Some(definition) = spec.spec.resources.get(&claim.resource) else {
            errors.push(ValidationError::MissingResourceReference {
                node_id: node.id.clone(),
                resource: claim.resource.clone(),
            });
            continue;
        };
        if claim.amount == 0 || claim.amount > definition.limit {
            errors.push(ValidationError::ResourceLimitExceeded {
                node_id: node.id.clone(),
                resource: claim.resource.clone(),
                amount: claim.amount,
                limit: definition.limit,
            });
        }
        if definition.kind == ResourceKind::ExclusiveLock && claim.amount != 1 {
            errors.push(ValidationError::ResourceLimitExceeded {
                node_id: node.id.clone(),
                resource: claim.resource.clone(),
                amount: claim.amount,
                limit: 1,
            });
        }
    }
}

fn validate_node_contract(node: &NodeSpec, warnings: &mut Vec<ValidationWarning>) {
    if node.is_model_worker() && node.objective.trim().is_empty() {
        warnings.push(ValidationWarning {
            code: "missing-objective",
            message: "model worker should have a bounded objective".to_string(),
            node_id: Some(node.id.clone()),
        });
    }
    if node.cache_policy == CachePolicy::ReadWrite && node.write_set.is_empty() {
        warnings.push(ValidationWarning {
            code: "cache-without-write-set",
            message: "read-write cache nodes should declare write scope".to_string(),
            node_id: Some(node.id.clone()),
        });
    }
}

fn validate_edges(
    spec: &GraphSpec,
    node_index: &BTreeMap<NodeId, &NodeSpec>,
    errors: &mut Vec<ValidationError>,
) {
    for edge in &spec.spec.edges {
        let edge_id = edge.stable_id();
        if !node_index.contains_key(&edge.from) {
            errors.push(ValidationError::MissingEdgeNodeReference {
                edge_id: edge_id.clone(),
                referenced: edge.from.clone(),
            });
        }
        if !node_index.contains_key(&edge.to) {
            errors.push(ValidationError::MissingEdgeNodeReference {
                edge_id: edge_id.clone(),
                referenced: edge.to.clone(),
            });
        }
        if edge.kind == EdgeKind::Data && edge.bindings.is_empty() {
            let target_has_input_from_source = node_index.get(&edge.to).is_some_and(|node| {
                node.inputs
                    .values()
                    .any(|input| input.from_node == edge.from)
            });
            if !target_has_input_from_source {
                errors.push(ValidationError::DataEdgeWithoutBinding {
                    edge_id: edge_id.clone(),
                });
            }
        }
        for binding in &edge.bindings {
            if let Some(schema_ref) = &binding.schema_ref {
                if !spec.spec.schemas.contains_key(schema_ref) {
                    errors.push(ValidationError::MissingSchemaReference {
                        node_id: edge.to.clone(),
                        schema_ref: schema_ref.clone(),
                    });
                }
            }
            if let Err(err) = validate_json_path(&binding.path) {
                errors.push(ValidationError::InvalidJsonPath {
                    node_id: edge.to.clone(),
                    path: binding.path.clone(),
                    reason: err.to_string(),
                });
            }
        }
        if let Some(join) = &edge.join {
            validate_join(spec, &edge.to, &edge_id, join, errors);
        }
        if let Some(condition) = &edge.condition {
            let known_nodes = node_index.keys().cloned().collect::<BTreeSet<_>>();
            if let Err(err) = validate_predicate(condition, &known_nodes) {
                errors.push(ValidationError::InvalidPredicate {
                    node_id: edge.to.clone(),
                    reason: err.to_string(),
                });
            }
        }
    }
}

fn validate_join(
    spec: &GraphSpec,
    target: &str,
    edge_id: &str,
    join: &JoinSpec,
    errors: &mut Vec<ValidationError>,
) {
    let upstream_width = upstream_width_for_target(spec, target);
    match join.policy {
        JoinPolicy::Quorum | JoinPolicy::MinimumSuccess => {
            let Some(required) = join.required else {
                errors.push(ValidationError::InvalidJoin {
                    edge_id: edge_id.to_string(),
                    reason: "required count is missing".to_string(),
                });
                return;
            };
            if required == 0 {
                errors.push(ValidationError::InvalidJoin {
                    edge_id: edge_id.to_string(),
                    reason: "required count must be greater than zero".to_string(),
                });
            }
            if upstream_width > 0 && required > upstream_width {
                errors.push(ValidationError::InvalidJoin {
                    edge_id: edge_id.to_string(),
                    reason: format!(
                        "required count {required} exceeds upstream width {upstream_width}"
                    ),
                });
            }
        }
        JoinPolicy::AllSuccess
        | JoinPolicy::AllTerminal
        | JoinPolicy::AnySuccess
        | JoinPolicy::FirstValid
        | JoinPolicy::DeadlineBestEffort => {}
    }
}

fn upstream_width_for_target(spec: &GraphSpec, target: &str) -> u32 {
    let node_index = spec
        .spec
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    spec.spec
        .edges
        .iter()
        .filter(|edge| edge.to == target)
        .filter_map(|edge| node_index.get(edge.from.as_str()))
        .map(|node| node.expected_instance_count())
        .sum()
}

fn validate_write_conflicts(spec: &GraphSpec, errors: &mut Vec<ValidationError>) {
    let writers = spec
        .spec
        .nodes
        .iter()
        .filter(|node| {
            node.effective_capability(&spec.spec.defaults) == CapabilityMode::UnisolatedWrite
                && !node.write_set.is_empty()
        })
        .collect::<Vec<_>>();

    for (idx, left) in writers.iter().enumerate() {
        for right in writers.iter().skip(idx + 1) {
            for left_path in &left.write_set {
                for right_path in &right.write_set {
                    if path_patterns_overlap(left_path, right_path) {
                        errors.push(ValidationError::WriteSetConflict {
                            left: left.id.clone(),
                            right: right.id.clone(),
                            path: format!("{left_path} <-> {right_path}"),
                        });
                    }
                }
            }
        }
    }
}

fn path_patterns_overlap(left: &str, right: &str) -> bool {
    let left = write_prefix(left);
    let right = write_prefix(right);
    left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}

fn write_prefix(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_string()
}

fn validate_exact_ready(
    topology: &TopologyReport,
    options: &ValidationOptions,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(required) = options.require_exact_ready_agents {
        let actual = topology.initial_ready_agent_width;
        if actual != required {
            errors.push(ValidationError::ExactReadyMismatch { required, actual });
        }
    }
}

fn validate_id(field: &'static str, id: &str, errors: &mut Vec<ValidationError>) {
    if id.is_empty() {
        errors.push(ValidationError::InvalidId {
            field,
            id: id.to_string(),
            reason: "id cannot be empty".to_string(),
        });
        return;
    }
    let mut chars = id.chars();
    if !chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric()) {
        errors.push(ValidationError::InvalidId {
            field,
            id: id.to_string(),
            reason: "id must start with an ASCII letter or number".to_string(),
        });
        return;
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        errors.push(ValidationError::InvalidId {
            field,
            id: id.to_string(),
            reason: "id can contain only ASCII letters, numbers, dots, dashes, and underscores"
                .to_string(),
        });
    }
}

pub fn validate_node_output(output: &super::types::NodeOutput) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if output.schema_version != AGENTGRAPH_SCHEMA_VERSION {
        errors.push(ValidationError::UnsupportedSchemaVersion {
            found: output.schema_version,
        });
    }
    validate_id("graphRunId", &output.graph_run_id, &mut errors);
    validate_id("nodeInstanceId", &output.node_instance_id, &mut errors);
    validate_id("attemptId", &output.attempt_id, &mut errors);

    for path in output
        .files_read
        .iter()
        .chain(output.files_changed.iter())
        .chain(output.artifacts.iter().map(|artifact| &artifact.path))
        .chain(
            output
                .findings
                .iter()
                .flat_map(|finding| finding.evidence.iter().map(|evidence| &evidence.path)),
        )
    {
        if let Err(reason) = validate_path_pattern(path, &[]) {
            errors.push(ValidationError::InvalidPath {
                node_id: output.node_instance_id.clone(),
                path: path.clone(),
                reason,
            });
        }
    }

    errors
}

pub fn collect_statuses(
    outputs: &[super::types::NodeOutput],
) -> BTreeMap<NodeId, super::types::NodeStatus> {
    outputs
        .iter()
        .map(|output| (output.node_instance_id.clone(), output.status))
        .collect()
}

pub fn output_as_json(output: &super::types::NodeOutput) -> Result<JsonValue, serde_json::Error> {
    serde_json::to_value(output)
}
