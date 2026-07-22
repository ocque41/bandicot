use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub const AGENTGRAPH_API_VERSION: &str = "bandicot.dev/v1alpha1";
pub const AGENTGRAPH_KIND: &str = "AgentGraph";
pub const AGENTGRAPH_SCHEMA_VERSION: u32 = 1;

pub type NodeId = String;
pub type EdgeId = String;
pub type SchemaId = String;
pub type ResourceId = String;
pub type ToolName = String;
pub type JsonPath = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSpec {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: GraphMetadata,
    pub spec: GraphSpecBody,
}

impl GraphSpec {
    pub fn schema_version(&self) -> u32 {
        self.metadata.graph_version
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphMetadata {
    pub name: String,
    pub graph_version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSpecBody {
    pub objective: String,
    #[serde(default)]
    pub execution: ExecutionPolicy,
    #[serde(default)]
    pub budgets: GraphBudgets,
    #[serde(default)]
    pub defaults: NodeDefaults,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub schemas: BTreeMap<SchemaId, JsonValue>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub templates: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub model_selectors: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub credential_refs: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<ResourceId, ResourceDefinition>,
    #[serde(default)]
    pub nodes: Vec<NodeSpec>,
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPolicy {
    #[serde(default)]
    pub orchestration_policy: OrchestrationMode,
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    #[serde(default = "default_max_total_nodes")]
    pub max_total_nodes: u32,
    #[serde(default = "default_max_active_model_calls")]
    pub max_active_model_calls: u32,
    #[serde(default = "default_true")]
    pub disable_nested_bandicot_agents: bool,
    #[serde(default = "default_true")]
    pub disable_provider_multi_agent_for_workers: bool,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            orchestration_policy: OrchestrationMode::Standard,
            max_depth: default_max_depth(),
            max_total_nodes: default_max_total_nodes(),
            max_active_model_calls: default_max_active_model_calls(),
            disable_nested_bandicot_agents: true,
            disable_provider_multi_agent_for_workers: true,
        }
    }
}

fn default_max_depth() -> u32 {
    1
}

fn default_max_total_nodes() -> u32 {
    160
}

fn default_max_active_model_calls() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphBudgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_time_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_model_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_node_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_generated_dynamic_nodes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loop_iterations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_estimated_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_provider_reported_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_failures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_failure_percentage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rate_limited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rate_limited_percentage: Option<f64>,
    #[serde(default)]
    pub hard_stop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selector: Option<String>,
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub service_tier: ServiceTierPreference,
    #[serde(default)]
    pub capability_mode: CapabilityMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

impl Default for NodeDefaults {
    fn default() -> Self {
        Self {
            model_selector: None,
            reasoning_effort: ReasoningEffort::Low,
            service_tier: ServiceTierPreference::Inherit,
            capability_mode: CapabilityMode::ReadOnly,
            timeout_seconds: None,
            max_tool_calls: None,
            max_input_tokens: None,
            max_output_tokens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    pub id: NodeId,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub objective: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definition_of_done: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_ref: Option<SchemaId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_set: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_set: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_effects: Vec<ExternalEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_mode: Option<CapabilityMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_allowlist: Vec<ToolName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_denylist: Vec<ToolName>,
    #[serde(default)]
    pub network_policy: NetworkPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTierPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_deadline_seconds: Option<u64>,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    #[serde(default)]
    pub idempotency_policy: IdempotencyPolicy,
    #[serde(default)]
    pub cache_policy: CachePolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_claims: Vec<ResourceClaim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_requirements: Vec<EvidenceRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification_requirements: Vec<VerificationRequirement>,
    #[serde(default)]
    pub failure_policy: FailurePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<MapSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce: Option<ReduceSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_policy: Option<LoopSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<RouteSpec>,
}

impl NodeSpec {
    pub fn effective_capability(&self, defaults: &NodeDefaults) -> CapabilityMode {
        self.capability_mode.unwrap_or(defaults.capability_mode)
    }

    pub fn effective_service_tier(&self, defaults: &NodeDefaults) -> ServiceTierPreference {
        self.service_tier.unwrap_or(defaults.service_tier)
    }

    pub fn expected_instance_count(&self) -> u32 {
        match (&self.kind, &self.map) {
            (NodeKind::MapAgent, Some(map)) => map
                .expected_instances
                .or(map.max_generated_instances)
                .unwrap_or(1),
            _ => 1,
        }
    }

    pub fn is_model_worker(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::Agent
                | NodeKind::MapAgent
                | NodeKind::ReduceAgent
                | NodeKind::Verifier
                | NodeKind::Router
                | NodeKind::Loop
                | NodeKind::Compensation
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputBinding {
    pub from_node: NodeId,
    pub path: JsonPath,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<SchemaId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<EdgeId>,
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<EdgeBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<JoinSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Predicate>,
}

impl EdgeSpec {
    pub fn stable_id(&self) -> EdgeId {
        self.id
            .clone()
            .unwrap_or_else(|| format!("{}--{}--{:?}", self.from, self.to, self.kind))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeBinding {
    pub input: String,
    pub path: JsonPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<SchemaId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinSpec {
    pub policy: JoinPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSpec {
    pub from_node: NodeId,
    pub path: JsonPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_instances: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_generated_instances: Option<u32>,
    #[serde(default)]
    pub deterministic_instance_ids: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_schema_ref: Option<SchemaId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_ref: Option<SchemaId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReduceSpec {
    pub from_node: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_in: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoopSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_generated_nodes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_model_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_time_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_progress_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deduplication_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_predicate: Option<Predicate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSpec {
    pub predicate: Predicate,
    pub to: NodeId,
    #[serde(default)]
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Predicate {
    Equals {
        path: JsonPath,
        value: JsonValue,
    },
    NotEquals {
        path: JsonPath,
        value: JsonValue,
    },
    Exists {
        path: JsonPath,
    },
    GreaterThan {
        path: JsonPath,
        value: JsonNumber,
    },
    LessThan {
        path: JsonPath,
        value: JsonNumber,
    },
    In {
        path: JsonPath,
        values: Vec<JsonValue>,
    },
    And {
        predicates: Vec<Predicate>,
    },
    Or {
        predicates: Vec<Predicate>,
    },
    Not {
        predicate: Box<Predicate>,
    },
    StatusIs {
        node_id: NodeId,
        status: NodeStatus,
    },
    SuccessCountAtLeast {
        node_set: Vec<NodeId>,
        count: u32,
    },
    DeadlineReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonNumber(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OrchestrationMode {
    #[default]
    Standard,
    Ultra,
    Swarm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityMode {
    #[default]
    ReadOnly,
    WorktreeWrite,
    UnisolatedWrite,
    ExternalEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceTierPreference {
    #[default]
    Inherit,
    Standard,
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectiveServiceTier {
    Standard,
    Priority,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedServiceTier {
    pub requested: ServiceTierPreference,
    pub effective: EffectiveServiceTier,
    pub source: SettingSource,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SettingSource {
    #[default]
    Default,
    Config,
    Session,
    Graph,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    #[default]
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    Deterministic,
    Agent,
    MapAgent,
    ReduceAgent,
    Verifier,
    Router,
    Barrier,
    Approval,
    Compensation,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Data,
    Control,
    Conditional,
    Failure,
    Verification,
    Approval,
    Compensation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JoinPolicy {
    AllSuccess,
    AllTerminal,
    AnySuccess,
    FirstValid,
    Quorum,
    MinimumSuccess,
    DeadlineBestEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeStatus {
    Pending,
    Blocked,
    Ready,
    Leased,
    Starting,
    Running,
    WaitingForApproval,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Skipped,
    Stale,
    Retrying,
    Compensating,
}

impl NodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            NodeStatus::Succeeded
                | NodeStatus::Failed
                | NodeStatus::TimedOut
                | NodeStatus::Cancelled
                | NodeStatus::Skipped
                | NodeStatus::Stale
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Draft,
    Validating,
    Validated,
    AwaitingApproval,
    Running,
    Pausing,
    Paused,
    Draining,
    Reducing,
    Verifying,
    Completed,
    PartiallyCompleted,
    Failed,
    BudgetStopped,
    Cancelled,
    Compensating,
    Compensated,
    CompensationFailed,
    ManualInterventionRequired,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAccounting {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub model_calls: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub node_attempts: u64,
    #[serde(default)]
    pub generated_dynamic_nodes: u64,
    #[serde(default)]
    pub loop_iterations: u64,
    #[serde(default)]
    pub estimated_cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reported_cost_usd: Option<f64>,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub rate_limited: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalEffect {
    FileWrite,
    Network,
    Database,
    Deployment,
    Publication,
    CredentialChange,
    DestructiveAction,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    #[serde(default)]
    pub mode: NetworkMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Denied,
            allowed_hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    #[default]
    Denied,
    Allowlisted,
    Unrestricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    #[serde(default)]
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_backoff_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_to_close_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_retry_delay_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_equivalent_failures: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_no_progress_retries: Option<u32>,
    #[serde(default = "default_retry_jitter_percent")]
    pub jitter_percent: u8,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_seconds: None,
            max_backoff_seconds: None,
            schedule_to_close_seconds: None,
            max_total_retry_delay_seconds: None,
            max_equivalent_failures: None,
            max_no_progress_retries: None,
            jitter_percent: default_retry_jitter_percent(),
        }
    }
}

fn default_retry_jitter_percent() -> u8 {
    20
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IdempotencyPolicy {
    #[default]
    Idempotent,
    NonIdempotent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CachePolicy {
    #[default]
    Disabled,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FailurePolicy {
    #[default]
    FailGraph,
    Continue,
    SkipDownstream,
    Compensate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDefinition {
    pub kind: ResourceKind,
    #[serde(default = "default_resource_limit")]
    pub limit: u32,
}

fn default_resource_limit() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    CountedPermit,
    ExclusiveLock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceClaim {
    pub resource: ResourceId,
    #[serde(default = "default_resource_amount")]
    pub amount: u32,
}

fn default_resource_amount() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRequirement {
    pub kind: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationRequirement {
    pub kind: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeOutput {
    pub schema_version: u32,
    pub graph_run_id: String,
    pub node_instance_id: String,
    pub attempt_id: String,
    pub status: NodeStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<NodeFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands_run: Vec<CommandRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests_run: Vec<TestRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeFinding {
    pub claim: String,
    pub severity: FindingSeverity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRun {
    pub command: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub command: String,
    pub result: TestResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestResult {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}
