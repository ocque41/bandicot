use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::api_key_provider::ApiKeyProvider;

/// Auth scheme for API key authentication.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    None,
    #[default]
    Bearer,
    XApiKey,
}

/// A single provider entry in the fallback chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// Human-readable name for this provider (e.g., "zen-account-a").
    pub name: String,
    /// The API key for this provider.
    pub api_key: String,
    /// Optional different base URL for this provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Auth scheme to use (Bearer or XApiKey).
    #[serde(default)]
    pub auth_scheme: AuthScheme,
}

/// Configuration for the fallback provider chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Ordered list of providers to try.
    pub providers: Vec<ProviderEntry>,
    /// How long to wait before retrying a failed provider (in seconds).
    #[serde(default = "default_recovery_interval_secs")]
    pub recovery_interval_secs: u64,
    /// Maximum number of consecutive failures before marking a provider as unhealthy.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,
}

fn default_recovery_interval_secs() -> u64 {
    300 // 5 minutes
}

fn default_max_failures() -> u32 {
    3
}

/// Provider health status tracked internally.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub consecutive_failures: u32,
    pub last_failure: Option<Instant>,
    pub last_success: Option<Instant>,
    pub is_circuit_open: bool,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure: None,
            last_success: None,
            is_circuit_open: false,
        }
    }
}

/// A fallback API key provider that chains multiple providers.
///
/// When a provider fails (rate limit, auth error, etc.), the system
/// automatically moves to the next provider in the chain. Providers
/// are periodically retried after a recovery interval.
///
/// # Example
///
/// ```rust
/// use xai_grok_tools::types::fallback_provider::{FallbackApiKeyProvider, ProviderEntry, AuthScheme};
///
/// let provider = FallbackApiKeyProvider::new(vec![
///     ProviderEntry {
///         name: "zen-a".to_string(),
///         api_key: "sk-primary".to_string(),
///         base_url: None,
///         auth_scheme: AuthScheme::Bearer,
///     },
///     ProviderEntry {
///         name: "go-a".to_string(),
///         api_key: "sk-fallback".to_string(),
///         base_url: None,
///         auth_scheme: AuthScheme::Bearer,
///     },
/// ]);
/// ```
pub struct FallbackApiKeyProvider {
    providers: Vec<ProviderEntry>,
    current_index: AtomicUsize,
    health: Mutex<HashMap<usize, ProviderHealth>>,
    recovery_interval: Duration,
    max_failures: u32,
}

impl FallbackApiKeyProvider {
    /// Create a new fallback provider with default settings.
    pub fn new(providers: Vec<ProviderEntry>) -> Self {
        Self::with_config(FallbackConfig {
            providers,
            recovery_interval_secs: default_recovery_interval_secs(),
            max_failures: default_max_failures(),
        })
    }

    /// Create a new fallback provider with custom configuration.
    pub fn with_config(config: FallbackConfig) -> Self {
        Self {
            providers: config.providers,
            current_index: AtomicUsize::new(0),
            health: Mutex::new(HashMap::new()),
            recovery_interval: Duration::from_secs(config.recovery_interval_secs),
            max_failures: config.max_failures,
        }
    }

    /// Parse configuration from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let config: FallbackConfig = serde_json::from_str(json)?;
        Ok(Self::with_config(config))
    }

    /// Parse configuration from environment variable `GROK_FALLBACK_KEYS`.
    pub fn from_env() -> Option<Self> {
        std::env::var("GROK_FALLBACK_KEYS")
            .ok()
            .and_then(|json| match Self::from_json(&json) {
                Ok(provider) => {
                    info!(
                        target: "fallback_provider",
                        provider_count = provider.providers.len(),
                        "Loaded fallback provider chain from environment"
                    );
                    Some(provider)
                }
                Err(e) => {
                    warn!(
                        target: "fallback_provider",
                        error = %e,
                        "Failed to parse GROK_FALLBACK_KEYS environment variable"
                    );
                    None
                }
            })
    }

    /// Get the current provider index.
    pub fn current_index(&self) -> usize {
        self.current_index.load(Ordering::Acquire)
    }

    /// Get the total number of providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Get the name of the current provider.
    pub fn current_provider_name(&self) -> Option<&str> {
        let idx = self.current_index();
        self.providers.get(idx).map(|p| p.name.as_str())
    }

    /// Get the current API key.
    fn get_current_key(&self) -> Option<String> {
        let idx = self.current_index();
        self.providers.get(idx).map(|p| p.api_key.clone())
    }

    /// Resolve the next available API key, skipping unhealthy providers.
    async fn resolve_next_key(&self) -> Option<String> {
        let mut health = self.health.lock().unwrap();
        let now = Instant::now();

        // Check if current provider is healthy or should be skipped
        if let Some(status) = health.get(&self.current_index()) {
            if status.is_circuit_open {
                // Try to recover if enough time has passed
                if let Some(last_failure) = status.last_failure {
                    if now.duration_since(last_failure) >= self.recovery_interval {
                        info!(
                            target: "fallback_provider",
                            provider_index = self.current_index(),
                            "Attempting recovery for circuit-open provider"
                        );
                        health.entry(self.current_index()).and_modify(|h| {
                            h.is_circuit_open = false;
                            h.consecutive_failures = 0;
                        });
                    } else {
                        // Move to next provider
                        self.move_to_next_provider();
                    }
                }
            }
        }

        // Find the next healthy provider
        let start_index = self.current_index();
        let mut attempts = 0;
        while attempts < self.providers.len() {
            let idx = self.current_index();
            let is_healthy = health
                .get(&idx)
                .map(|h| !h.is_circuit_open && h.consecutive_failures < self.max_failures)
                .unwrap_or(true);

            if is_healthy {
                return self.providers.get(idx).map(|p| p.api_key.clone());
            }

            self.move_to_next_provider();
            attempts += 1;

            // If we've wrapped around, break to avoid infinite loop
            if self.current_index() == start_index {
                break;
            }
        }

        // All providers are unhealthy, return the current one anyway
        warn!(
            target: "fallback_provider",
            "All providers are unhealthy, returning current provider anyway"
        );
        self.providers
            .get(self.current_index())
            .map(|p| p.api_key.clone())
    }

    /// Move to the next provider in the chain.
    fn move_to_next_provider(&self) {
        let current = self.current_index.load(Ordering::Acquire);
        let next = (current + 1) % self.providers.len();
        self.current_index.store(next, Ordering::Release);

        info!(
            target: "fallback_provider",
            from_index = current,
            to_index = next,
            from_provider = %self.providers[current].name,
            to_provider = %self.providers[next].name,
            "Fallback: switching to next provider"
        );
    }

    /// Record a successful request for the current provider.
    pub fn record_success(&self) {
        let idx = self.current_index();
        let mut health = self.health.lock().unwrap();
        let entry = health.entry(idx).or_default();
        entry.consecutive_failures = 0;
        entry.last_success = Some(Instant::now());
        entry.is_circuit_open = false;
    }

    /// Record a failed request for the current provider.
    ///
    /// If the failure is a rate limit (429) or auth error (401/403),
    /// the provider is marked as unhealthy and the next provider is selected.
    pub fn record_failure(&self, is_rate_limit: bool, is_auth_error: bool) {
        let idx = self.current_index();
        let mut health = self.health.lock().unwrap();
        let entry = health.entry(idx).or_default();
        entry.consecutive_failures += 1;
        entry.last_failure = Some(Instant::now());

        // Open circuit breaker for rate limits and auth errors
        if is_rate_limit || is_auth_error || entry.consecutive_failures >= self.max_failures {
            entry.is_circuit_open = true;
            warn!(
                target: "fallback_provider",
                provider_index = idx,
                provider_name = %self.providers[idx].name,
                consecutive_failures = entry.consecutive_failures,
                is_rate_limit,
                is_auth_error,
                "Provider circuit breaker opened"
            );

            // Move to next provider
            drop(health);
            self.move_to_next_provider();
        }
    }

    /// Get health status for all providers (for debugging/telemetry).
    pub fn health_status(&self) -> Vec<(String, ProviderHealth)> {
        let health = self.health.lock().unwrap();
        self.providers
            .iter()
            .enumerate()
            .map(|(idx, provider)| {
                let status = health.get(&idx).cloned().unwrap_or_default();
                (provider.name.clone(), status)
            })
            .collect()
    }
}

impl std::fmt::Debug for FallbackApiKeyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackApiKeyProvider")
            .field("provider_count", &self.providers.len())
            .field("current_index", &self.current_index())
            .field("current_provider", &self.current_provider_name())
            .finish()
    }
}

impl ApiKeyProvider for FallbackApiKeyProvider {
    fn current_api_key(&self) -> Option<String> {
        self.get_current_key()
    }

    fn current_api_key_async(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(self.resolve_next_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_providers() -> Vec<ProviderEntry> {
        vec![
            ProviderEntry {
                name: "primary".to_string(),
                api_key: "sk-primary".to_string(),
                base_url: None,
                auth_scheme: AuthScheme::Bearer,
            },
            ProviderEntry {
                name: "fallback-1".to_string(),
                api_key: "sk-fallback-1".to_string(),
                base_url: None,
                auth_scheme: AuthScheme::Bearer,
            },
            ProviderEntry {
                name: "fallback-2".to_string(),
                api_key: "sk-fallback-2".to_string(),
                base_url: None,
                auth_scheme: AuthScheme::Bearer,
            },
        ]
    }

    #[test]
    fn test_new_provider() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        assert_eq!(provider.provider_count(), 3);
        assert_eq!(provider.current_index(), 0);
        assert_eq!(provider.current_provider_name(), Some("primary"));
    }

    #[test]
    fn test_get_current_key() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        assert_eq!(provider.current_api_key(), Some("sk-primary".to_string()));
    }

    #[test]
    fn test_move_to_next_provider() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.move_to_next_provider();
        assert_eq!(provider.current_index(), 1);
        assert_eq!(provider.current_provider_name(), Some("fallback-1"));
    }

    #[test]
    fn test_move_to_next_provider_wraps_around() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.move_to_next_provider();
        provider.move_to_next_provider();
        provider.move_to_next_provider(); // Should wrap to 0
        assert_eq!(provider.current_index(), 0);
    }

    #[test]
    fn test_record_success() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.record_success();
        let health = provider.health_status();
        assert_eq!(health[0].1.consecutive_failures, 0);
    }

    #[test]
    fn test_record_failure_opens_circuit() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.record_failure(true, false); // Rate limit
        let health = provider.health_status();
        assert!(health[0].1.is_circuit_open);
        assert_eq!(provider.current_index(), 1); // Moved to next
    }

    #[test]
    fn test_record_failure_auth_error_opens_circuit() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.record_failure(false, true); // Auth error
        let health = provider.health_status();
        assert!(health[0].1.is_circuit_open);
        assert_eq!(provider.current_index(), 1);
    }

    #[test]
    fn test_consecutive_failures_opens_circuit() {
        let config = FallbackConfig {
            providers: test_providers(),
            recovery_interval_secs: 300,
            max_failures: 2,
        };
        let provider = FallbackApiKeyProvider::with_config(config);

        provider.record_failure(false, false); // First failure
        assert_eq!(provider.current_index(), 0); // Still on same provider

        provider.record_failure(false, false); // Second failure - should open circuit
        assert_eq!(provider.current_index(), 1); // Moved to next
    }

    #[test]
    fn test_from_json() {
        let json = r#"{
            "providers": [
                {"name": "a", "api_key": "sk-a"},
                {"name": "b", "api_key": "sk-b"}
            ],
            "recovery_interval_secs": 600,
            "max_failures": 5
        }"#;

        let provider = FallbackApiKeyProvider::from_json(json).unwrap();
        assert_eq!(provider.provider_count(), 2);
        assert_eq!(provider.recovery_interval, Duration::from_secs(600));
        assert_eq!(provider.max_failures, 5);
    }

    #[test]
    fn test_from_json_defaults() {
        let json = r#"{
            "providers": [
                {"name": "a", "api_key": "sk-a"}
            ]
        }"#;

        let provider = FallbackApiKeyProvider::from_json(json).unwrap();
        assert_eq!(provider.recovery_interval, Duration::from_secs(300));
        assert_eq!(provider.max_failures, 3);
    }

    #[tokio::test]
    async fn test_resolve_next_key_healthy_provider() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        let key = provider.resolve_next_key().await;
        assert_eq!(key, Some("sk-primary".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_next_key_skips_unhealthy() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        provider.record_failure(true, false); // Mark primary as unhealthy

        let key = provider.resolve_next_key().await;
        assert_eq!(key, Some("sk-fallback-1".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_next_key_all_unhealthy_returns_current() {
        let provider = FallbackApiKeyProvider::new(test_providers());

        // Mark all providers as unhealthy
        for _ in 0..3 {
            provider.record_failure(true, false);
        }

        // Should still return a key (the current one)
        let key = provider.resolve_next_key().await;
        assert!(key.is_some());
    }

    #[test]
    fn test_auth_scheme_serialization() {
        let scheme = AuthScheme::Bearer;
        let json = serde_json::to_string(&scheme).unwrap();
        assert_eq!(json, "\"bearer\"");

        let scheme = AuthScheme::XApiKey;
        let json = serde_json::to_string(&scheme).unwrap();
        assert_eq!(json, "\"x_api_key\"");
    }

    #[test]
    fn test_provider_entry_serialization() {
        let entry = ProviderEntry {
            name: "test".to_string(),
            api_key: "sk-test".to_string(),
            base_url: Some("https://api.example.com".to_string()),
            auth_scheme: AuthScheme::Bearer,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"api_key\":\"sk-test\""));
        assert!(json.contains("\"base_url\":\"https://api.example.com\""));
    }
}
