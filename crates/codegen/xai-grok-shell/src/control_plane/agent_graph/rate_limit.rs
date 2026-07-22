use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use xai_grok_sampling_types::RateLimitMetadata;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRouteKey {
    pub provider_id: String,
    pub shared_limit_group: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapacityConfig {
    pub conservative_request_limit: u64,
    pub conservative_token_limit: u64,
    pub safety_headroom: f64,
    pub recovery_step: u64,
    pub authentication_failure_threshold: u32,
}

impl Default for ProviderCapacityConfig {
    fn default() -> Self {
        Self {
            conservative_request_limit: 4,
            conservative_token_limit: 100_000,
            safety_headroom: 0.10,
            recovery_step: 1,
            authentication_failure_threshold: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacitySource {
    Estimated,
    ProviderHeaders,
    ObservedBackoff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapacityState {
    pub request_limit: u64,
    pub remaining_requests: u64,
    pub token_limit: u64,
    pub remaining_tokens: u64,
    pub reset_at_ms: Option<i64>,
    pub blocked_until_ms: Option<i64>,
    pub active_requests: u64,
    pub reserved_tokens: u64,
    pub source: CapacitySource,
    pub consecutive_auth_failures: u32,
    pub circuit_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapacityObservation {
    pub route: ProviderRouteKey,
    pub rate_limits: RateLimitMetadata,
}

static SESSION_OBSERVATIONS: LazyLock<Mutex<BTreeMap<String, ProviderCapacityObservation>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub fn route_for_base_url(base_url: &str) -> ProviderRouteKey {
    let authority_with_userinfo = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or("unknown");
    let authority = authority_with_userinfo
        .rsplit_once('@')
        .map_or(authority_with_userinfo, |(_, host)| host)
        .to_ascii_lowercase();
    ProviderRouteKey {
        provider_id: authority.clone(),
        // A host-level bucket is deliberately conservative. It never guesses
        // that two accounts have separate capacity and contains no credential.
        shared_limit_group: authority,
    }
}

pub fn record_session_observation(
    session_id: &str,
    base_url: &str,
    rate_limits: RateLimitMetadata,
) {
    if !session_id.starts_with("agentgraph-") {
        return;
    }
    let observation = ProviderCapacityObservation {
        route: route_for_base_url(base_url),
        rate_limits,
    };
    let mut observations = SESSION_OBSERVATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    observations.insert(session_id.to_string(), observation);
    while observations.len() > 2_048 {
        let Some(oldest_key) = observations.keys().next().cloned() else {
            break;
        };
        observations.remove(&oldest_key);
    }
}

pub fn take_session_observation(session_id: &str) -> Option<ProviderCapacityObservation> {
    SESSION_OBSERVATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .cloned()
}

pub fn observed_capacity_snapshot() -> Vec<ProviderCapacityObservation> {
    let observations = SESSION_OBSERVATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut by_route = BTreeMap::new();
    for observation in observations.values() {
        by_route.insert(observation.route.clone(), observation.clone());
    }
    by_route.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPermit {
    route: ProviderRouteKey,
    reserved_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAdmission {
    Admitted(ProviderPermit),
    WaitUntil(i64),
    Saturated,
    CircuitOpen,
}

#[derive(Debug, Default)]
pub struct ProviderCapacityController {
    routes: BTreeMap<ProviderRouteKey, (ProviderCapacityConfig, ProviderCapacityState)>,
}

impl ProviderCapacityController {
    pub fn configure(&mut self, route: ProviderRouteKey, config: ProviderCapacityConfig) {
        let state = ProviderCapacityState {
            request_limit: config.conservative_request_limit.max(1),
            remaining_requests: config.conservative_request_limit.max(1),
            token_limit: config.conservative_token_limit.max(1),
            remaining_tokens: config.conservative_token_limit.max(1),
            reset_at_ms: None,
            blocked_until_ms: None,
            active_requests: 0,
            reserved_tokens: 0,
            source: CapacitySource::Estimated,
            consecutive_auth_failures: 0,
            circuit_open: false,
        };
        self.routes.insert(route, (config, state));
    }

    pub fn reserve(
        &mut self,
        route: &ProviderRouteKey,
        estimated_tokens: u64,
        now_ms: i64,
    ) -> ProviderAdmission {
        let Some((_, state)) = self.routes.get_mut(route) else {
            return ProviderAdmission::Saturated;
        };
        recover_window(state, now_ms);
        if state.circuit_open {
            return ProviderAdmission::CircuitOpen;
        }
        if let Some(deadline) = state.blocked_until_ms.filter(|deadline| *deadline > now_ms) {
            return ProviderAdmission::WaitUntil(deadline);
        }
        if state.active_requests >= state.remaining_requests
            || estimated_tokens > state.remaining_tokens.saturating_sub(state.reserved_tokens)
        {
            return state
                .reset_at_ms
                .filter(|deadline| *deadline > now_ms)
                .map_or(ProviderAdmission::Saturated, ProviderAdmission::WaitUntil);
        }
        state.active_requests += 1;
        state.reserved_tokens = state.reserved_tokens.saturating_add(estimated_tokens);
        ProviderAdmission::Admitted(ProviderPermit {
            route: route.clone(),
            reserved_tokens: estimated_tokens,
        })
    }

    pub fn release(&mut self, permit: ProviderPermit, actual_tokens: Option<u64>) {
        let Some((_, state)) = self.routes.get_mut(&permit.route) else {
            return;
        };
        state.active_requests = state.active_requests.saturating_sub(1);
        state.reserved_tokens = state.reserved_tokens.saturating_sub(permit.reserved_tokens);
        state.remaining_requests = state.remaining_requests.saturating_sub(1);
        state.remaining_tokens = state
            .remaining_tokens
            .saturating_sub(actual_tokens.unwrap_or(permit.reserved_tokens));
    }

    pub fn observe_headers(
        &mut self,
        route: &ProviderRouteKey,
        metadata: &RateLimitMetadata,
        now_ms: i64,
    ) {
        let Some((config, state)) = self.routes.get_mut(route) else {
            return;
        };
        let headroom = |value: u64| {
            ((value as f64) * (1.0 - config.safety_headroom.clamp(0.0, 0.9))).floor() as u64
        };
        if let Some(limit) = metadata.requests.limit {
            state.request_limit = headroom(limit).max(1);
        }
        if let Some(remaining) = metadata.requests.remaining {
            state.remaining_requests = headroom(remaining).min(state.request_limit);
        }
        if let Some(limit) = metadata.tokens.limit.or(metadata.project_tokens.limit) {
            state.token_limit = headroom(limit).max(1);
        }
        if let Some(remaining) = metadata
            .tokens
            .remaining
            .or(metadata.project_tokens.remaining)
        {
            state.remaining_tokens = headroom(remaining).min(state.token_limit);
        }
        let reset_after = metadata
            .requests
            .reset_after_ms
            .into_iter()
            .chain(metadata.tokens.reset_after_ms)
            .chain(metadata.project_tokens.reset_after_ms)
            .max();
        state.reset_at_ms = reset_after.map(|delay| now_ms.saturating_add(delay as i64));
        state.source = CapacitySource::ProviderHeaders;
        state.consecutive_auth_failures = 0;
    }

    pub fn observe_rate_limited(
        &mut self,
        route: &ProviderRouteKey,
        retry_after_ms: Option<u64>,
        now_ms: i64,
    ) {
        let Some((_, state)) = self.routes.get_mut(route) else {
            return;
        };
        state.request_limit = (state.request_limit / 2).max(1);
        state.remaining_requests = 0;
        state.blocked_until_ms =
            Some(now_ms.saturating_add(retry_after_ms.unwrap_or(1_000) as i64));
        state.source = CapacitySource::ObservedBackoff;
    }

    pub fn observe_auth_failure(&mut self, route: &ProviderRouteKey) {
        let Some((config, state)) = self.routes.get_mut(route) else {
            return;
        };
        state.consecutive_auth_failures = state.consecutive_auth_failures.saturating_add(1);
        if state.consecutive_auth_failures >= config.authentication_failure_threshold.max(1) {
            state.circuit_open = true;
        }
    }

    pub fn state(&self, route: &ProviderRouteKey) -> Option<&ProviderCapacityState> {
        self.routes.get(route).map(|(_, state)| state)
    }

    pub fn contains_route(&self, route: &ProviderRouteKey) -> bool {
        self.routes.contains_key(route)
    }

    pub fn tick(&mut self, now_ms: i64) {
        for (config, state) in self.routes.values_mut() {
            if state
                .blocked_until_ms
                .is_some_and(|deadline| deadline <= now_ms)
            {
                state.blocked_until_ms = None;
                state.remaining_requests = state
                    .remaining_requests
                    .saturating_add(config.recovery_step.max(1))
                    .min(state.request_limit);
            }
            recover_window(state, now_ms);
        }
    }
}

fn recover_window(state: &mut ProviderCapacityState, now_ms: i64) {
    if state.reset_at_ms.is_some_and(|deadline| deadline <= now_ms) {
        state.remaining_requests = state.request_limit;
        state.remaining_tokens = state.token_limit;
        state.reset_at_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::RateLimitWindow;

    fn route() -> ProviderRouteKey {
        ProviderRouteKey {
            provider_id: "provider-a".into(),
            shared_limit_group: "account-1".into(),
        }
    }

    #[test]
    fn reserves_tokens_atomically_and_honors_reset() {
        let route = route();
        let mut controller = ProviderCapacityController::default();
        controller.configure(
            route.clone(),
            ProviderCapacityConfig {
                conservative_request_limit: 2,
                conservative_token_limit: 100,
                ..Default::default()
            },
        );
        let first = match controller.reserve(&route, 70, 0) {
            ProviderAdmission::Admitted(permit) => permit,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            controller.reserve(&route, 40, 0),
            ProviderAdmission::Saturated
        );
        controller.release(first, Some(60));
        controller.observe_headers(
            &route,
            &RateLimitMetadata {
                requests: RateLimitWindow {
                    limit: Some(10),
                    remaining: Some(0),
                    reset_after_ms: Some(500),
                },
                ..Default::default()
            },
            100,
        );
        assert_eq!(
            controller.reserve(&route, 1, 100),
            ProviderAdmission::WaitUntil(600)
        );
        controller.tick(600);
        assert!(matches!(
            controller.reserve(&route, 1, 600),
            ProviderAdmission::Admitted(_)
        ));
    }

    #[test]
    fn rate_limit_reduces_capacity_and_recovers_without_a_storm() {
        let route = route();
        let mut controller = ProviderCapacityController::default();
        controller.configure(route.clone(), ProviderCapacityConfig::default());
        controller.observe_rate_limited(&route, Some(1_000), 10);
        assert_eq!(
            controller.reserve(&route, 1, 100),
            ProviderAdmission::WaitUntil(1_010)
        );
        controller.tick(1_010);
        assert!(matches!(
            controller.reserve(&route, 1, 1_010),
            ProviderAdmission::Admitted(_)
        ));
        assert_eq!(
            controller.reserve(&route, 1, 1_010),
            ProviderAdmission::Saturated
        );
    }

    #[test]
    fn systemic_auth_failures_open_the_circuit() {
        let route = route();
        let mut controller = ProviderCapacityController::default();
        controller.configure(route.clone(), ProviderCapacityConfig::default());
        for _ in 0..3 {
            controller.observe_auth_failure(&route);
        }
        assert_eq!(
            controller.reserve(&route, 1, 0),
            ProviderAdmission::CircuitOpen
        );
    }
}
