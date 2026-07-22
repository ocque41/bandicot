use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::predicate::{PredicateContext, PredicateError, evaluate_predicate, get_json_path};
use super::types::{ArtifactRef, LoopSpec, NodeOutput, NodeStatus, UsageAccounting};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoopStatus {
    Running,
    Converged,
    Dry,
    NoProgress,
    IterationLimit,
    GeneratedNodeLimit,
    BudgetStopped,
    WallTimeStopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopState {
    pub iteration: u32,
    pub generated_node_count: u32,
    /// Stable identities for every unique dynamic expansion discovered so far.
    #[serde(default)]
    pub generated_node_ids: Vec<String>,
    pub seen: BTreeSet<String>,
    pub progress_value: u64,
    pub dry_round_count: u32,
    pub accumulated_usage: UsageAccounting,
    pub accumulated_artifacts: Vec<ArtifactRef>,
    pub terminal_predicate_result: bool,
    pub status: LoopStatus,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

impl LoopState {
    pub fn new(started_at_ms: i64) -> Self {
        Self {
            iteration: 0,
            generated_node_count: 0,
            generated_node_ids: Vec::new(),
            seen: BTreeSet::new(),
            progress_value: 0,
            dry_round_count: 0,
            accumulated_usage: UsageAccounting::default(),
            accumulated_artifacts: Vec::new(),
            terminal_predicate_result: false,
            status: LoopStatus::Running,
            started_at_ms,
            updated_at_ms: started_at_ms,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.status != LoopStatus::Running
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDecision {
    Continue,
    Complete,
    Partial,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("loop policy is missing")]
    MissingPolicy,
    #[error("loop predicate failed: {0}")]
    Predicate(#[from] PredicateError),
    #[error("loop output could not be serialized: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn advance_loop(
    policy: Option<&LoopSpec>,
    state: &mut LoopState,
    output: &NodeOutput,
    usage: &UsageAccounting,
    statuses: &BTreeMap<String, NodeStatus>,
    at_ms: i64,
) -> Result<LoopDecision, LoopError> {
    let policy = policy.ok_or(LoopError::MissingPolicy)?;
    state.iteration = state.iteration.saturating_add(1);
    state.updated_at_ms = at_ms;
    state.accumulated_usage = super::budget::add_usage(&state.accumulated_usage, usage);
    for artifact in &output.artifacts {
        if !state
            .accumulated_artifacts
            .iter()
            .any(|existing| existing.path == artifact.path && existing.sha256 == artifact.sha256)
        {
            state.accumulated_artifacts.push(artifact.clone());
        }
    }

    let output_json = serde_json::to_value(output)?;
    let before = state.seen.len();
    for finding in &output.findings {
        let finding_json = serde_json::to_value(finding)?;
        let key = policy
            .deduplication_key
            .as_deref()
            .and_then(|path| get_json_path(&finding_json, path).ok().flatten())
            .map(canonical_value)
            .unwrap_or_else(|| canonical_value(&finding_json));
        if state.seen.insert(key.clone()) {
            state.generated_node_ids.push(dynamic_node_id(
                &output.node_instance_id,
                state.iteration,
                &key,
            ));
        }
    }
    let new_items = state.seen.len().saturating_sub(before) as u32;
    state.generated_node_count = state.generated_node_count.saturating_add(new_items);
    let progress = progress_value(policy.progress_metric.as_deref(), &output_json, state);
    if new_items == 0 || progress <= state.progress_value {
        state.dry_round_count = state.dry_round_count.saturating_add(1);
    } else {
        state.dry_round_count = 0;
    }
    state.progress_value = state.progress_value.max(progress);

    let deadline_reached = policy.max_wall_time_seconds.is_some_and(|seconds| {
        at_ms
            >= state
                .started_at_ms
                .saturating_add(seconds.saturating_mul(1_000) as i64)
    });
    state.terminal_predicate_result = if let Some(predicate) = &policy.terminal_predicate {
        evaluate_predicate(
            predicate,
            &PredicateContext {
                document: &output_json,
                statuses,
                deadline_reached,
            },
        )?
    } else {
        false
    };

    if state.terminal_predicate_result {
        state.status = LoopStatus::Converged;
        return Ok(LoopDecision::Complete);
    }
    if deadline_reached {
        state.status = LoopStatus::WallTimeStopped;
        return Ok(LoopDecision::Partial);
    }
    if exceeds_usage(
        policy.max_input_tokens,
        state.accumulated_usage.input_tokens,
    ) || exceeds_usage(
        policy.max_output_tokens,
        state.accumulated_usage.output_tokens,
    ) || exceeds_usage(policy.max_model_calls, state.accumulated_usage.model_calls)
    {
        state.status = LoopStatus::BudgetStopped;
        return Ok(LoopDecision::Partial);
    }
    if policy
        .max_generated_nodes
        .is_some_and(|limit| state.generated_node_count >= limit)
    {
        state.status = LoopStatus::GeneratedNodeLimit;
        return Ok(LoopDecision::Partial);
    }
    if policy
        .no_progress_limit
        .is_some_and(|limit| state.dry_round_count >= limit)
    {
        state.status = if new_items == 0 {
            LoopStatus::Dry
        } else {
            LoopStatus::NoProgress
        };
        return Ok(LoopDecision::Complete);
    }
    if policy
        .max_iterations
        .is_some_and(|limit| state.iteration >= limit)
    {
        state.status = LoopStatus::IterationLimit;
        return Ok(LoopDecision::Partial);
    }
    state.status = LoopStatus::Running;
    Ok(LoopDecision::Continue)
}

pub fn dynamic_node_id(loop_node_id: &str, iteration: u32, dedup_key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(loop_node_id.as_bytes());
    hash.update([0]);
    hash.update(iteration.to_le_bytes());
    hash.update([0]);
    hash.update(dedup_key.as_bytes());
    let digest = hash.finalize();
    let short = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{loop_node_id}-iteration-{iteration:04}-{short}")
}

fn progress_value(metric: Option<&str>, output: &serde_json::Value, state: &LoopState) -> u64 {
    match metric.unwrap_or("unique-findings") {
        "unique-findings" | "seen-count" => state.seen.len() as u64,
        path => get_json_path(output, path)
            .ok()
            .flatten()
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(state.seen.len() as u64),
    }
}

fn canonical_value(value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn exceeds_usage(limit: Option<u64>, actual: u64) -> bool {
    limit.is_some_and(|limit| actual >= limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::agent_graph::types::{FindingSeverity, NodeFinding};

    fn output(claims: &[&str]) -> NodeOutput {
        NodeOutput {
            schema_version: 1,
            graph_run_id: "run".to_string(),
            node_instance_id: "loop".to_string(),
            attempt_id: "attempt".to_string(),
            status: NodeStatus::Succeeded,
            summary: "round".to_string(),
            findings: claims
                .iter()
                .map(|claim| NodeFinding {
                    claim: (*claim).to_string(),
                    severity: FindingSeverity::Low,
                    evidence: Vec::new(),
                })
                .collect(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            tests_run: Vec::new(),
            artifacts: Vec::new(),
            assumptions: Vec::new(),
            blockers: Vec::new(),
            confidence: 1.0,
        }
    }

    fn policy() -> LoopSpec {
        LoopSpec {
            max_iterations: Some(5),
            max_generated_nodes: Some(20),
            max_input_tokens: Some(1_000),
            max_output_tokens: Some(1_000),
            max_model_calls: Some(10),
            max_wall_time_seconds: Some(60),
            progress_metric: Some("unique-findings".to_string()),
            no_progress_limit: Some(2),
            deduplication_key: Some("$.claim".to_string()),
            terminal_predicate: None,
        }
    }

    #[test]
    fn duplicate_findings_are_seen_and_dry_rounds_terminate() {
        let mut state = LoopState::new(0);
        let usage = UsageAccounting {
            model_calls: 1,
            ..UsageAccounting::default()
        };
        let statuses = BTreeMap::new();
        assert_eq!(
            advance_loop(
                Some(&policy()),
                &mut state,
                &output(&["a"]),
                &usage,
                &statuses,
                1
            )
            .expect("round"),
            LoopDecision::Continue
        );
        assert_eq!(
            advance_loop(
                Some(&policy()),
                &mut state,
                &output(&["a"]),
                &usage,
                &statuses,
                2
            )
            .expect("round"),
            LoopDecision::Continue
        );
        assert_eq!(
            advance_loop(
                Some(&policy()),
                &mut state,
                &output(&["a"]),
                &usage,
                &statuses,
                3
            )
            .expect("round"),
            LoopDecision::Complete
        );
        assert_eq!(state.seen.len(), 1);
        assert_eq!(state.status, LoopStatus::Dry);
    }

    #[test]
    fn dynamic_ids_are_deterministic() {
        assert_eq!(
            dynamic_node_id("loop", 2, "finding-a"),
            dynamic_node_id("loop", 2, "finding-a")
        );
        assert_ne!(
            dynamic_node_id("loop", 2, "finding-a"),
            dynamic_node_id("loop", 3, "finding-a")
        );
    }
}
