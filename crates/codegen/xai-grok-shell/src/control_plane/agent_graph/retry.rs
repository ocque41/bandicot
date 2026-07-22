use serde::{Deserialize, Serialize};

use super::types::{IdempotencyPolicy, NodeOutput, NodeSpec, RetryPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureClassification {
    RateLimited,
    BadGateway,
    ServiceUnavailable,
    GatewayTimeout,
    TemporaryConnection,
    StreamDisconnected,
    WorkerProcessFailed,
    StartTimeout,
    RetryableSchemaRepair,
    BadRequest,
    Unauthorized,
    Forbidden,
    InvalidModel,
    UnsupportedCapability,
    InvalidGraphContract,
    PermissionDenied,
    BudgetExhausted,
    PolicyRejected,
    UnsafeNonIdempotentUncertainty,
    UserCancelled,
    CompensationFailed,
    ServiceTierRejected,
    Unknown,
}

impl FailureClassification {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::BadGateway
                | Self::ServiceUnavailable
                | Self::GatewayTimeout
                | Self::TemporaryConnection
                | Self::StreamDisconnected
                | Self::WorkerProcessFailed
                | Self::StartTimeout
                | Self::RetryableSchemaRepair
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySchedule {
    pub run_id: String,
    pub node_id: String,
    pub prior_attempt: u32,
    pub next_attempt: u32,
    pub classification: FailureClassification,
    pub chosen_delay_ms: u64,
    pub next_attempt_at_ms: i64,
    pub retry_after_ms: Option<u64>,
    pub jitter_ms: i64,
    pub equivalent_failure_count: u32,
    pub no_progress_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    Schedule(RetrySchedule),
    Stop { reason: String },
}

pub fn classify_failure_text(text: &str) -> FailureClassification {
    let value = text.to_ascii_lowercase();
    if value.contains("service_tier") || value.contains("service tier") {
        return FailureClassification::ServiceTierRejected;
    }
    if value.contains("429") || value.contains("rate limit") || value.contains("too many requests")
    {
        return FailureClassification::RateLimited;
    }
    if value.contains("502") || value.contains("bad gateway") {
        return FailureClassification::BadGateway;
    }
    if value.contains("503") || value.contains("service unavailable") {
        return FailureClassification::ServiceUnavailable;
    }
    if value.contains("504") || value.contains("gateway timeout") {
        return FailureClassification::GatewayTimeout;
    }
    if value.contains("401") || value.contains("unauthorized") {
        return FailureClassification::Unauthorized;
    }
    if value.contains("403") || value.contains("forbidden") {
        return FailureClassification::Forbidden;
    }
    if value.contains("400") || value.contains("bad request") {
        return FailureClassification::BadRequest;
    }
    if value.contains("invalid model") || value.contains("model not found") {
        return FailureClassification::InvalidModel;
    }
    if value.contains("unsupported capability") {
        return FailureClassification::UnsupportedCapability;
    }
    if value.contains("permission denied") || value.contains("approval denied") {
        return FailureClassification::PermissionDenied;
    }
    if value.contains("budget") && (value.contains("exhaust") || value.contains("exceed")) {
        return FailureClassification::BudgetExhausted;
    }
    if value.contains("cancelled") || value.contains("canceled") {
        return FailureClassification::UserCancelled;
    }
    if value.contains("schema") && value.contains("retryable") {
        return FailureClassification::RetryableSchemaRepair;
    }
    if value.contains("stream") && (value.contains("disconnect") || value.contains("closed")) {
        return FailureClassification::StreamDisconnected;
    }
    if value.contains("start timeout") || value.contains("timed out before start") {
        return FailureClassification::StartTimeout;
    }
    if value.contains("connection") || value.contains("temporarily unavailable") {
        return FailureClassification::TemporaryConnection;
    }
    if value.contains("process exit") || value.contains("worker failed") {
        return FailureClassification::WorkerProcessFailed;
    }
    FailureClassification::Unknown
}

pub fn classify_node_output(output: &NodeOutput) -> FailureClassification {
    let combined = std::iter::once(output.summary.as_str())
        .chain(output.blockers.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    classify_failure_text(&combined)
}

#[allow(clippy::too_many_arguments)]
pub fn decide_retry(
    run_id: &str,
    node: &NodeSpec,
    prior_attempt: u32,
    classification: FailureClassification,
    now_ms: i64,
    run_started_at_ms: i64,
    accumulated_retry_delay_ms: u64,
    equivalent_failure_count: u32,
    no_progress_count: u32,
    retry_after_ms: Option<u64>,
) -> RetryDecision {
    let policy = &node.retry_policy;
    if classification == FailureClassification::ServiceTierRejected {
        return stop(
            "service-tier rejection is handled only by the typed one-time Standard fallback",
        );
    }
    if !classification.retryable() {
        return stop("failure classification is not retryable");
    }
    if node.idempotency_policy == IdempotencyPolicy::NonIdempotent {
        return stop("non-idempotent uncertain work requires inspection before retry");
    }
    if prior_attempt >= policy.max_attempts.max(1) {
        return stop("maximum attempts reached");
    }
    if policy
        .max_equivalent_failures
        .is_some_and(|limit| equivalent_failure_count >= limit)
    {
        return stop("equivalent-failure limit reached");
    }
    if policy
        .max_no_progress_retries
        .is_some_and(|limit| no_progress_count >= limit)
    {
        return stop("no-progress retry limit reached");
    }

    let base_ms = policy.backoff_seconds.unwrap_or(1).saturating_mul(1_000);
    let exponent = prior_attempt.saturating_sub(1).min(20);
    let exponential_ms = base_ms.saturating_mul(1_u64 << exponent);
    let max_ms = policy
        .max_backoff_seconds
        .unwrap_or(300)
        .saturating_mul(1_000);
    let capped_ms = exponential_ms.min(max_ms);
    let jitter_limit = capped_ms.saturating_mul(u64::from(policy.jitter_percent.min(100))) / 100;
    let jitter_ms = deterministic_jitter(run_id, &node.id, prior_attempt, jitter_limit);
    let randomized_ms = apply_signed_jitter(capped_ms, jitter_ms);
    let chosen_delay_ms = randomized_ms.max(retry_after_ms.unwrap_or(0));

    if policy.max_total_retry_delay_seconds.is_some_and(|limit| {
        accumulated_retry_delay_ms.saturating_add(chosen_delay_ms) > limit.saturating_mul(1_000)
    }) {
        return stop("total retry-delay limit would be exceeded");
    }
    let next_attempt_at_ms = now_ms.saturating_add(chosen_delay_ms.min(i64::MAX as u64) as i64);
    if policy.schedule_to_close_seconds.is_some_and(|limit| {
        next_attempt_at_ms > run_started_at_ms.saturating_add(limit.saturating_mul(1_000) as i64)
    }) {
        return stop("schedule-to-close deadline would be exceeded");
    }

    RetryDecision::Schedule(RetrySchedule {
        run_id: run_id.to_string(),
        node_id: node.id.clone(),
        prior_attempt,
        next_attempt: prior_attempt.saturating_add(1),
        classification,
        chosen_delay_ms,
        next_attempt_at_ms,
        retry_after_ms,
        jitter_ms,
        equivalent_failure_count: equivalent_failure_count.saturating_add(1),
        no_progress_count: no_progress_count.saturating_add(1),
    })
}

fn stop(reason: &str) -> RetryDecision {
    RetryDecision::Stop {
        reason: reason.to_string(),
    }
}

fn deterministic_jitter(run_id: &str, node_id: &str, attempt: u32, limit: u64) -> i64 {
    if limit == 0 {
        return 0;
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in run_id
        .bytes()
        .chain(node_id.bytes())
        .chain(attempt.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let width = limit.saturating_mul(2).saturating_add(1);
    (hash % width) as i64 - limit as i64
}

fn apply_signed_jitter(base: u64, jitter: i64) -> u64 {
    if jitter < 0 {
        base.saturating_sub(jitter.unsigned_abs())
    } else {
        base.saturating_add(jitter as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::agent_graph::types::{NodeKind, NodeSpec};

    fn node() -> NodeSpec {
        serde_json::from_value(serde_json::json!({
            "id": "worker",
            "kind": "agent",
            "objective": "work",
            "retryPolicy": {
                "maxAttempts": 3,
                "backoffSeconds": 2,
                "maxBackoffSeconds": 30,
                "maxTotalRetryDelaySeconds": 60,
                "jitterPercent": 20
            }
        }))
        .expect("node")
    }

    #[test]
    fn retry_deadline_is_deterministic_and_persistable() {
        let node = node();
        assert_eq!(node.kind, NodeKind::Agent);
        let first = decide_retry(
            "run",
            &node,
            1,
            FailureClassification::RateLimited,
            10_000,
            1_000,
            0,
            0,
            0,
            Some(3_000),
        );
        let second = decide_retry(
            "run",
            &node,
            1,
            FailureClassification::RateLimited,
            10_000,
            1_000,
            0,
            0,
            0,
            Some(3_000),
        );
        assert_eq!(first, second);
        let RetryDecision::Schedule(schedule) = first else {
            panic!("expected schedule");
        };
        assert!(schedule.chosen_delay_ms >= 3_000);
        assert_eq!(schedule.next_attempt, 2);
    }

    #[test]
    fn auth_and_service_tier_failures_do_not_enter_generic_retry() {
        let node = node();
        for classification in [
            FailureClassification::Unauthorized,
            FailureClassification::Forbidden,
            FailureClassification::ServiceTierRejected,
        ] {
            assert!(matches!(
                decide_retry("run", &node, 1, classification, 0, 0, 0, 0, 0, None),
                RetryDecision::Stop { .. }
            ));
        }
    }
}
