use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc as StdArc;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend as ExistingSubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::{
    ModelOverrideProvenance, SubagentCancelOutcome, SubagentOwner, SubagentRequest, SubagentResult,
    SubagentRuntimeOverrides, SubagentSnapshotStatus, SubagentValidateTypeOutcome,
};
use xai_tool_types::{
    SubagentCapabilityMode, SubagentIsolationMode, SubagentServiceTierPreference,
};

use super::model_selector::{builtin_selector, resolve_live_selector};
use super::resources::is_nested_orchestration_tool;
use super::types::{
    AGENTGRAPH_SCHEMA_VERSION, CapabilityMode, NodeDefaults, NodeId, NodeOutput, NodeSpec,
    NodeStatus, ReasoningEffort, ServiceTierPreference as GraphServiceTierPreference,
    UsageAccounting,
};
use super::verification::verify_node_output;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub graph_run_id: String,
    pub node_id: NodeId,
    pub attempt: u32,
    pub objective: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerCompletion {
    pub graph_run_id: String,
    pub node_id: NodeId,
    pub attempt: u32,
    pub output: NodeOutput,
    #[serde(default)]
    pub usage: UsageAccounting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_capacity: Option<super::rate_limit::ProviderCapacityObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHandle {
    pub worker_id: String,
    pub node_id: NodeId,
    pub attempt: u32,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerError {
    #[error("worker `{worker_id}` was not found")]
    NotFound { worker_id: String },
    #[error("worker backend rejected node `{node_id}`: {reason}")]
    Rejected { node_id: NodeId, reason: String },
}

pub trait WorkerBackend {
    fn start(&mut self, request: WorkerRequest) -> Result<WorkerHandle, WorkerError>;
    fn poll_completed(&mut self) -> Vec<WorkerCompletion>;
    fn cancel(&mut self, worker_id: &str) -> Result<(), WorkerError>;
    fn active_count(&self) -> usize;
}

#[derive(Clone)]
pub struct SubagentWorkerBackend {
    backend: Arc<dyn ExistingSubagentBackend>,
    parent_session_id: String,
    parent_prompt_id: Option<String>,
    cwd: PathBuf,
    models_manager: Option<crate::agent::models::ModelsManager>,
}

impl std::fmt::Debug for SubagentWorkerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentWorkerBackend")
            .field("parent_session_id", &self.parent_session_id)
            .field("parent_prompt_id", &self.parent_prompt_id)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl SubagentWorkerBackend {
    pub fn new(
        backend: Arc<dyn ExistingSubagentBackend>,
        parent_session_id: impl Into<String>,
        cwd: impl AsRef<Path>,
    ) -> Self {
        Self {
            backend,
            parent_session_id: parent_session_id.into(),
            parent_prompt_id: None,
            cwd: cwd.as_ref().to_path_buf(),
            models_manager: None,
        }
    }

    pub fn with_models_manager(
        mut self,
        models_manager: crate::agent::models::ModelsManager,
    ) -> Self {
        self.models_manager = Some(models_manager);
        self
    }

    pub fn with_parent_prompt_id(mut self, parent_prompt_id: Option<String>) -> Self {
        self.parent_prompt_id = parent_prompt_id;
        self
    }

    pub async fn run_node(
        &self,
        graph_run_id: &str,
        node: &NodeSpec,
        defaults: &NodeDefaults,
        schemas: &BTreeMap<String, JsonValue>,
        attempt: u32,
        worker_id: String,
    ) -> Result<WorkerCompletion, WorkerError> {
        self.validate_node_authority(node, defaults)?;
        let subagent_type = subagent_type_for_node(node);
        self.validate_subagent_type(node, &subagent_type).await?;

        let request = self.subagent_request(
            graph_run_id,
            node,
            defaults,
            attempt,
            worker_id.clone(),
            subagent_type,
        )?;
        let result = self
            .backend
            .spawn(request)
            .await
            .map_err(|err| WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("subagent transport failed: {err}"),
            })?;
        let result = if result.backgrounded {
            match self
                .resolve_backgrounded_result(&worker_id, node, defaults)
                .await?
            {
                Some(result) => result,
                None => {
                    return Ok(timed_out_completion(
                        graph_run_id,
                        &node.id,
                        attempt,
                        "subagent exceeded the node timeout before returning terminal output",
                    ));
                }
            }
        } else {
            result
        };

        let child_session_id = result.child_session_id.clone();
        let mut completion = completion_from_subagent_result(
            graph_run_id,
            node,
            schemas,
            &self.cwd,
            attempt,
            result,
        );
        completion.provider_capacity =
            super::rate_limit::take_session_observation(&child_session_id);
        Ok(completion)
    }

    pub async fn cancel(&self, worker_id: &str) -> Result<(), WorkerError> {
        match self.backend.cancel(worker_id).await {
            SubagentCancelOutcome::Cancelled | SubagentCancelOutcome::AlreadyFinished { .. } => {
                Ok(())
            }
            SubagentCancelOutcome::NotFound => Err(WorkerError::NotFound {
                worker_id: worker_id.to_string(),
            }),
        }
    }

    fn validate_node_authority(
        &self,
        node: &NodeSpec,
        defaults: &NodeDefaults,
    ) -> Result<(), WorkerError> {
        if !node.is_model_worker() {
            return Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: "only model worker nodes can be dispatched to subagents".to_string(),
            });
        }

        for tool in &node.tool_allowlist {
            if is_nested_orchestration_tool(tool) {
                return Err(WorkerError::Rejected {
                    node_id: node.id.clone(),
                    reason: format!("nested orchestration tool `{tool}` is not allowed"),
                });
            }
        }

        let capability = node.effective_capability(defaults);
        if matches!(
            capability,
            CapabilityMode::UnisolatedWrite | CapabilityMode::ExternalEffect
        ) {
            return Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!(
                    "local AgentGraph workers require read-only or isolated-worktree authority; node requested {capability:?}"
                ),
            });
        }

        if let Some(selector) = node
            .model_selector
            .as_deref()
            .or(defaults.model_selector.as_deref())
            && selector != "inherit"
            && builtin_selector(selector).is_none()
        {
            return Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("model selector `{selector}` has no supported candidate"),
            });
        }

        Ok(())
    }

    async fn validate_subagent_type(
        &self,
        node: &NodeSpec,
        subagent_type: &str,
    ) -> Result<(), WorkerError> {
        match self
            .backend
            .validate_type(subagent_type, &self.parent_session_id)
            .await
        {
            SubagentValidateTypeOutcome::Ok => Ok(()),
            SubagentValidateTypeOutcome::Unknown { available } => Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: if available.is_empty() {
                    format!("unknown subagent type `{subagent_type}`")
                } else {
                    format!(
                        "unknown subagent type `{subagent_type}`; available types: {}",
                        available.join(", ")
                    )
                },
            }),
            SubagentValidateTypeOutcome::Disabled => Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("subagent type `{subagent_type}` is disabled"),
            }),
            SubagentValidateTypeOutcome::NotAllowed { allowed } => Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!(
                    "subagent type `{subagent_type}` is not allowed; allowed types: {}",
                    allowed.join(", ")
                ),
            }),
            SubagentValidateTypeOutcome::ValidationUnavailable => Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("cannot validate subagent type `{subagent_type}`"),
            }),
            _ => Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!(
                    "subagent type `{subagent_type}` returned an unknown validation outcome"
                ),
            }),
        }
    }

    fn subagent_request(
        &self,
        graph_run_id: &str,
        node: &NodeSpec,
        defaults: &NodeDefaults,
        attempt: u32,
        worker_id: String,
        subagent_type: String,
    ) -> Result<SubagentRequest, WorkerError> {
        let (result_tx, _) = oneshot::channel();
        let model = self.resolve_model(node, defaults)?;
        Ok(SubagentRequest {
            id: worker_id,
            prompt: graph_worker_prompt(graph_run_id, node, defaults, attempt),
            description: graph_worker_description(node),
            subagent_type,
            parent_session_id: self.parent_session_id.clone(),
            parent_prompt_id: self.parent_prompt_id.clone(),
            resume_from: None,
            cwd: Some(self.cwd.display().to_string()),
            runtime_overrides: SubagentRuntimeOverrides {
                model,
                model_override_provenance: ModelOverrideProvenance::Harness,
                reasoning_effort: Some(reasoning_effort_name(
                    node.reasoning_effort.unwrap_or(defaults.reasoning_effort),
                )),
                persona: None,
                capability_mode: Some(match node.effective_capability(defaults) {
                    CapabilityMode::ReadOnly => SubagentCapabilityMode::ReadOnly,
                    CapabilityMode::WorktreeWrite => SubagentCapabilityMode::ReadWrite,
                    CapabilityMode::UnisolatedWrite | CapabilityMode::ExternalEffect => {
                        SubagentCapabilityMode::ReadOnly
                    }
                }),
                isolation: Some(match node.effective_capability(defaults) {
                    CapabilityMode::WorktreeWrite => SubagentIsolationMode::Worktree,
                    _ => SubagentIsolationMode::None,
                }),
                service_tier: Some(graph_service_tier_to_subagent(
                    node.effective_service_tier(defaults),
                )),
                hosted_multi_agent: Some(false),
                harness_agent_type: None,
                completion_output_cap: None,
                spawn_depth: None,
                output_token_budget: None,
                output_schema: None,
                loop_task_id: None,
            },
            run_in_background: false,
            surface_completion: false,
            await_to_completion: false,
            fork_context: false,
            owner: SubagentOwner::Task,
            cancel_token: CancellationToken::new(),
            result_tx,
        })
    }

    fn resolve_model(
        &self,
        node: &NodeSpec,
        defaults: &NodeDefaults,
    ) -> Result<Option<String>, WorkerError> {
        let Some(selector_name) = node
            .model_selector
            .as_deref()
            .or(defaults.model_selector.as_deref())
        else {
            return Ok(None);
        };
        let Some(selector) = builtin_selector(selector_name) else {
            return Err(WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("unknown model selector `{selector_name}`"),
            });
        };
        if selector.allow_inherit {
            return Ok(None);
        }
        if let Some(manager) = &self.models_manager {
            return resolve_live_selector(
                manager,
                &selector,
                node.reasoning_effort.unwrap_or(defaults.reasoning_effort),
                node.effective_service_tier(defaults),
            )
            .map(|resolution| Some(resolution.selected_model))
            .map_err(|err| WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("model selector `{selector_name}` did not resolve: {err:?}"),
            });
        }
        // Compatibility for embedders that have not supplied a catalog. The
        // production command path always supplies ModelsManager and therefore
        // never assumes these models exist.
        Ok(selector
            .candidates
            .first()
            .map(|candidate| candidate.model_slug.clone()))
    }

    pub fn provider_route_for_node(
        &self,
        node: &NodeSpec,
        defaults: &NodeDefaults,
    ) -> Result<Option<super::rate_limit::ProviderRouteKey>, WorkerError> {
        let Some(manager) = &self.models_manager else {
            return Ok(None);
        };
        let Some(model_id) = self.resolve_model(node, defaults)? else {
            return Ok(None);
        };
        let catalog = manager.agent_graph_catalog();
        let model = catalog
            .iter()
            .find(|model| model.model == model_id || model.id.as_deref() == Some(model_id.as_str()))
            .ok_or_else(|| WorkerError::Rejected {
                node_id: node.id.clone(),
                reason: format!("resolved model `{model_id}` disappeared from the live catalog"),
            })?;
        Ok(Some(super::rate_limit::route_for_base_url(&model.base_url)))
    }

    async fn resolve_backgrounded_result(
        &self,
        worker_id: &str,
        node: &NodeSpec,
        defaults: &NodeDefaults,
    ) -> Result<Option<SubagentResult>, WorkerError> {
        let timeout_ms = node
            .timeout_seconds
            .or(defaults.timeout_seconds)
            .unwrap_or(600)
            .saturating_mul(1000);
        let Some(snapshot) = self.backend.query(worker_id, true, Some(timeout_ms)).await else {
            let _ = self.backend.cancel(worker_id).await;
            return Ok(None);
        };
        match snapshot.status {
            SubagentSnapshotStatus::Completed {
                output,
                tool_calls,
                turns,
                worktree_path,
            } => Ok(Some(SubagentResult {
                success: true,
                output: StdArc::from(output),
                error: None,
                cancelled: false,
                subagent_id: snapshot.subagent_id,
                child_session_id: worker_id.to_string(),
                tool_calls,
                turns,
                duration_ms: snapshot.duration_ms,
                tokens_used: 0,
                output_tokens_used: 0,
                total_tokens_used: 0,
                output_usage_incomplete: false,
                worktree_path,
                backgrounded: false,
            })),
            SubagentSnapshotStatus::Failed { error } => Ok(Some(SubagentResult {
                success: false,
                output: StdArc::from(""),
                error: Some(error),
                cancelled: false,
                subagent_id: snapshot.subagent_id,
                child_session_id: worker_id.to_string(),
                tool_calls: 0,
                turns: 0,
                duration_ms: snapshot.duration_ms,
                tokens_used: 0,
                output_tokens_used: 0,
                total_tokens_used: 0,
                output_usage_incomplete: false,
                worktree_path: None,
                backgrounded: false,
            })),
            SubagentSnapshotStatus::Cancelled { reason } => Ok(Some(SubagentResult {
                success: false,
                output: StdArc::from(""),
                error: reason,
                cancelled: true,
                subagent_id: snapshot.subagent_id,
                child_session_id: worker_id.to_string(),
                tool_calls: 0,
                turns: 0,
                duration_ms: snapshot.duration_ms,
                tokens_used: 0,
                output_tokens_used: 0,
                total_tokens_used: 0,
                output_usage_incomplete: false,
                worktree_path: None,
                backgrounded: false,
            })),
            SubagentSnapshotStatus::Initializing | SubagentSnapshotStatus::Running { .. } => {
                let _ = self.backend.cancel(worker_id).await;
                Ok(None)
            }
        }
    }
}

pub fn graph_worker_id(run_id: &str, node_id: &str, attempt: u32) -> String {
    format!("agentgraph-{run_id}-{node_id}-{attempt}-{}", Uuid::now_v7())
}

fn graph_service_tier_to_subagent(
    preference: GraphServiceTierPreference,
) -> SubagentServiceTierPreference {
    match preference {
        GraphServiceTierPreference::Inherit => SubagentServiceTierPreference::Inherit,
        GraphServiceTierPreference::Standard => SubagentServiceTierPreference::Standard,
        GraphServiceTierPreference::Fast => SubagentServiceTierPreference::Fast,
    }
}

#[derive(Debug, Clone)]
pub struct FakeWorkerBackend {
    complete_immediately: bool,
    active: BTreeMap<String, WorkerRequest>,
    completions: VecDeque<WorkerCompletion>,
}

impl Default for FakeWorkerBackend {
    fn default() -> Self {
        Self::complete_immediately()
    }
}

impl FakeWorkerBackend {
    pub fn complete_immediately() -> Self {
        Self {
            complete_immediately: true,
            active: BTreeMap::new(),
            completions: VecDeque::new(),
        }
    }

    pub fn manual() -> Self {
        Self {
            complete_immediately: false,
            active: BTreeMap::new(),
            completions: VecDeque::new(),
        }
    }

    pub fn finish(&mut self, worker_id: &str, status: NodeStatus) -> Result<(), WorkerError> {
        let Some(request) = self.active.remove(worker_id) else {
            return Err(WorkerError::NotFound {
                worker_id: worker_id.to_string(),
            });
        };
        self.completions
            .push_back(completion_for_request(&request, status));
        Ok(())
    }
}

impl WorkerBackend for FakeWorkerBackend {
    fn start(&mut self, request: WorkerRequest) -> Result<WorkerHandle, WorkerError> {
        let worker_id = format!("{}-attempt-{}", request.node_id, request.attempt);
        let handle = WorkerHandle {
            worker_id: worker_id.clone(),
            node_id: request.node_id.clone(),
            attempt: request.attempt,
        };
        self.active.insert(worker_id.clone(), request.clone());
        if self.complete_immediately {
            self.finish(&worker_id, NodeStatus::Succeeded)?;
        }
        Ok(handle)
    }

    fn poll_completed(&mut self) -> Vec<WorkerCompletion> {
        self.completions.drain(..).collect()
    }

    fn cancel(&mut self, worker_id: &str) -> Result<(), WorkerError> {
        if self.active.remove(worker_id).is_some() {
            Ok(())
        } else {
            Err(WorkerError::NotFound {
                worker_id: worker_id.to_string(),
            })
        }
    }

    fn active_count(&self) -> usize {
        self.active.len()
    }
}

fn completion_for_request(request: &WorkerRequest, status: NodeStatus) -> WorkerCompletion {
    WorkerCompletion {
        graph_run_id: request.graph_run_id.clone(),
        node_id: request.node_id.clone(),
        attempt: request.attempt,
        output: NodeOutput {
            schema_version: AGENTGRAPH_SCHEMA_VERSION,
            graph_run_id: request.graph_run_id.clone(),
            node_instance_id: request.node_id.clone(),
            attempt_id: format!("attempt-{}", request.attempt),
            status,
            summary: format!("fake worker completed {}", request.node_id),
            findings: Vec::new(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            tests_run: Vec::new(),
            artifacts: Vec::new(),
            assumptions: Vec::new(),
            blockers: Vec::new(),
            confidence: 1.0,
        },
        usage: UsageAccounting {
            model_calls: 1,
            node_attempts: 1,
            ..UsageAccounting::default()
        },
        provider_capacity: None,
    }
}

fn completion_from_subagent_result(
    graph_run_id: &str,
    node: &NodeSpec,
    schemas: &BTreeMap<String, JsonValue>,
    repo_root: &Path,
    attempt: u32,
    result: SubagentResult,
) -> WorkerCompletion {
    let usage = UsageAccounting {
        input_tokens: result.tokens_used,
        model_calls: 1,
        tool_calls: result.tool_calls as u64,
        node_attempts: 1,
        ..UsageAccounting::default()
    };
    if result.cancelled {
        return terminal_text_completion(
            graph_run_id,
            &node.id,
            attempt,
            NodeStatus::Cancelled,
            result
                .error
                .as_deref()
                .unwrap_or("subagent was cancelled before producing output"),
        );
    }
    if !result.success {
        return terminal_text_completion(
            graph_run_id,
            &node.id,
            attempt,
            NodeStatus::Failed,
            result
                .error
                .as_deref()
                .unwrap_or("subagent failed before producing output"),
        );
    }

    let raw = result.output.trim();
    let parsed = serde_json::from_str::<NodeOutput>(raw);
    let mut output = match parsed {
        Ok(output) => output,
        Err(err) => {
            return terminal_text_completion(
                graph_run_id,
                &node.id,
                attempt,
                NodeStatus::Failed,
                &format!("subagent success output was not valid NodeOutput JSON: {err}"),
            );
        }
    };

    let mut blockers =
        validate_structured_node_output(graph_run_id, node, schemas, repo_root, attempt, &output);
    if !blockers.is_empty() && output.status == NodeStatus::Succeeded {
        output.status = NodeStatus::Failed;
        output.confidence = 0.0;
    }
    output.blockers.append(&mut blockers);

    WorkerCompletion {
        graph_run_id: graph_run_id.to_string(),
        node_id: node.id.clone(),
        attempt,
        output,
        usage,
        provider_capacity: None,
    }
}

fn terminal_text_completion(
    graph_run_id: &str,
    node_id: &str,
    attempt: u32,
    status: NodeStatus,
    summary: &str,
) -> WorkerCompletion {
    WorkerCompletion {
        graph_run_id: graph_run_id.to_string(),
        node_id: node_id.to_string(),
        attempt,
        output: NodeOutput {
            schema_version: AGENTGRAPH_SCHEMA_VERSION,
            graph_run_id: graph_run_id.to_string(),
            node_instance_id: node_id.to_string(),
            attempt_id: format!("attempt-{attempt}"),
            status,
            summary: summary.to_string(),
            findings: Vec::new(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            tests_run: Vec::new(),
            artifacts: Vec::new(),
            assumptions: Vec::new(),
            blockers: if status == NodeStatus::Succeeded {
                Vec::new()
            } else {
                vec![summary.to_string()]
            },
            confidence: if status == NodeStatus::Succeeded {
                1.0
            } else {
                0.0
            },
        },
        usage: UsageAccounting {
            model_calls: 1,
            node_attempts: 1,
            failures: u64::from(status != NodeStatus::Succeeded),
            ..UsageAccounting::default()
        },
        provider_capacity: None,
    }
}

fn timed_out_completion(
    graph_run_id: &str,
    node_id: &str,
    attempt: u32,
    summary: &str,
) -> WorkerCompletion {
    terminal_text_completion(
        graph_run_id,
        node_id,
        attempt,
        NodeStatus::TimedOut,
        summary,
    )
}

fn validate_structured_node_output(
    graph_run_id: &str,
    node: &NodeSpec,
    schemas: &BTreeMap<String, JsonValue>,
    repo_root: &Path,
    attempt: u32,
    output: &NodeOutput,
) -> Vec<String> {
    let mut blockers = Vec::new();
    if output.graph_run_id != graph_run_id {
        blockers.push(format!(
            "NodeOutput graphRunId `{}` did not match run `{graph_run_id}`",
            output.graph_run_id
        ));
    }
    if output.node_instance_id != node.id {
        blockers.push(format!(
            "NodeOutput nodeInstanceId `{}` did not match node `{}`",
            output.node_instance_id, node.id
        ));
    }
    let expected_attempt = format!("attempt-{attempt}");
    if output.attempt_id != expected_attempt {
        blockers.push(format!(
            "NodeOutput attemptId `{}` did not match `{expected_attempt}`",
            output.attempt_id
        ));
    }

    let verification = verify_node_output(output, repo_root, &node.write_set);
    blockers.extend(
        verification
            .errors
            .iter()
            .map(|error| format!("node output verification failed: {error}")),
    );
    blockers.extend(
        verification
            .claim_states
            .iter()
            .filter_map(|(claim, state)| {
                (*state == super::verification::ClaimState::Unverified)
                    .then(|| format!("claim `{claim}` is unverified"))
            }),
    );

    if let Some(schema) = node_output_schema(node, schemas) {
        match jsonschema::validator_for(schema) {
            Ok(validator) => match serde_json::to_value(output) {
                Ok(value) => {
                    if let Err(err) = validator.validate(&value) {
                        blockers.push(format!("node output does not match declared schema: {err}"));
                    }
                }
                Err(err) => blockers.push(format!("failed to serialize NodeOutput: {err}")),
            },
            Err(err) => blockers.push(format!("declared output schema is invalid: {err}")),
        }
    }

    blockers.extend(required_evidence_errors(node, output));
    blockers
}

fn node_output_schema<'a>(
    node: &'a NodeSpec,
    schemas: &'a BTreeMap<String, JsonValue>,
) -> Option<&'a JsonValue> {
    node.output_schema.as_ref().or_else(|| {
        node.output_schema_ref
            .as_ref()
            .and_then(|id| schemas.get(id))
    })
}

fn required_evidence_errors(node: &NodeSpec, output: &NodeOutput) -> Vec<String> {
    node.evidence_requirements
        .iter()
        .filter(|requirement| requirement.required)
        .filter_map(|requirement| {
            let kind = requirement.kind.trim();
            let satisfied = match kind {
                "node-output" => !output.summary.trim().is_empty(),
                "finding" | "findings" => !output.findings.is_empty(),
                "finding-evidence" => output
                    .findings
                    .iter()
                    .any(|finding| !finding.evidence.is_empty()),
                "artifact" | "artifacts" => !output.artifacts.is_empty(),
                _ => false,
            };
            (!satisfied).then(|| format!("required evidence `{kind}` was not satisfied"))
        })
        .collect()
}

fn subagent_type_for_node(node: &NodeSpec) -> String {
    for tag in &node.tags {
        if let Some(value) = tag
            .strip_prefix("subagent-type:")
            .or_else(|| tag.strip_prefix("subagent:"))
        {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "general-purpose".to_string()
}

fn graph_worker_description(node: &NodeSpec) -> String {
    let objective = node.objective.trim();
    if objective.is_empty() {
        format!("AgentGraph node {}", node.id)
    } else {
        format!("AgentGraph node {}: {}", node.id, objective)
    }
}

fn graph_worker_prompt(
    graph_run_id: &str,
    node: &NodeSpec,
    defaults: &NodeDefaults,
    attempt: u32,
) -> String {
    let definition_of_done = if node.definition_of_done.is_empty() {
        "- No explicit definition of done was provided.".to_string()
    } else {
        node.definition_of_done
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let inputs = serde_json::to_string_pretty(&node.inputs)
        .unwrap_or_else(|_| "<inputs unavailable>".to_string());
    let read_set = if node.read_set.is_empty() {
        "- No explicit read set was provided.".to_string()
    } else {
        node.read_set
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let write_set = if node.write_set.is_empty() {
        "- No writes are allowed.".to_string()
    } else {
        node.write_set
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "You are executing one AgentGraph worker node.\n\
         Graph run: {graph_run_id}\n\
         Node id: {node_id}\n\
         Attempt: {attempt}\n\
         Node kind: {node_kind:?}\n\
         Capability: {capability:?}\n\
         Objective:\n{objective}\n\n\
         Definition of done:\n{definition_of_done}\n\n\
         Inputs:\n{inputs}\n\n\
         Read set:\n{read_set}\n\n\
         Write set:\n{write_set}\n\n\
         Restrictions:\n\
         - Do not spawn child agents.\n\
         - Do not use /graph, /swarm, provider-hosted multi-agent features, or nested orchestration tools.\n\
         - Stay within the declared capability, read set, write set, network policy, and tool policy.\n\
         - Return only a single JSON object matching the AgentGraph NodeOutput schema.\n\
         - Set graphRunId to {graph_run_id}, nodeInstanceId to {node_id}, and attemptId to attempt-{attempt}.\n\
         - Do not wrap the JSON in Markdown or prose.",
        node_id = node.id,
        node_kind = node.kind,
        capability = node.effective_capability(defaults),
        objective = node.objective,
    )
}

fn reasoning_effort_name(reasoning_effort: ReasoningEffort) -> String {
    match reasoning_effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        ReasoningEffort::Max => "max",
    }
    .to_string()
}
