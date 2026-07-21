use std::sync::Arc;

use tracing::warn;

use super::api_key_provider::ApiKeyProvider;
use super::fallback_provider::FallbackApiKeyProvider;

/// A wrapper that connects `FallbackApiKeyProvider` to the session layer.
///
/// This struct provides:
/// - Thread-safe access to the fallback provider
/// - Success/failure recording for provider health tracking
/// - Integration with the existing `bearer_resolver` mechanism
///
/// # Example
///
/// ```rust
/// use xai_grok_tools::types::fallback_provider::FallbackApiKeyProvider;
/// use xai_grok_tools::types::fallback_wrapper::FallbackWrapper;
///
/// let provider = FallbackApiKeyProvider::new(vec![]);
/// let wrapper = FallbackWrapper::new(provider);
///
/// // Use in session
/// let key = wrapper.current_api_key();
/// wrapper.record_success();
/// ```
#[derive(Clone)]
pub struct FallbackWrapper {
    provider: Arc<FallbackApiKeyProvider>,
}

impl FallbackWrapper {
    /// Create a new fallback wrapper from a `FallbackApiKeyProvider`.
    pub fn new(provider: FallbackApiKeyProvider) -> Self {
        Self {
            provider: Arc::new(provider),
        }
    }

    /// Create from environment variable `GROK_FALLBACK_KEYS`.
    pub fn from_env() -> Option<Self> {
        FallbackApiKeyProvider::from_env().map(Self::new)
    }

    /// Create from JSON configuration string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        FallbackApiKeyProvider::from_json(json).map(Self::new)
    }

    /// Get the current API key.
    pub fn current_api_key(&self) -> Option<String> {
        self.provider.current_api_key()
    }

    /// Get the current provider name.
    pub fn current_provider_name(&self) -> Option<&str> {
        self.provider.current_provider_name()
    }

    /// Record a successful request for the current provider.
    pub fn record_success(&self) {
        self.provider.record_success();
    }

    /// Record a failed request for the current provider.
    ///
    /// If the failure is a rate limit (429) or auth error (401/403),
    /// the provider is marked as unhealthy and the next provider is selected.
    pub fn record_failure(&self, is_rate_limit: bool, is_auth_error: bool) {
        let prev_provider = self.provider.current_provider_name().map(String::from);
        self.provider.record_failure(is_rate_limit, is_auth_error);
        let new_provider = self.provider.current_provider_name();

        if prev_provider.as_deref() != new_provider {
            warn!(
                target: "fallback_wrapper",
                from_provider = ?prev_provider,
                to_provider = ?new_provider,
                is_rate_limit,
                is_auth_error,
                "Fallback: switching to next provider due to failure"
            );
        }
    }

    /// Get the underlying `FallbackApiKeyProvider` for advanced usage.
    pub fn provider(&self) -> &FallbackApiKeyProvider {
        &self.provider
    }

    /// Get health status for all providers (for debugging/telemetry).
    pub fn health_status(&self) -> Vec<(String, super::fallback_provider::ProviderHealth)> {
        self.provider.health_status()
    }
}

impl std::fmt::Debug for FallbackWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackWrapper")
            .field("provider", &self.provider)
            .finish()
    }
}

impl ApiKeyProvider for FallbackWrapper {
    fn current_api_key(&self) -> Option<String> {
        self.provider.current_api_key()
    }

    fn current_api_key_async(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<String>> + Send + '_>,
    > {
        self.provider.current_api_key_async()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fallback_provider::{AuthScheme, ProviderEntry};

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
        ]
    }

    #[test]
    fn test_new_wrapper() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        let wrapper = FallbackWrapper::new(provider);
        assert_eq!(wrapper.current_api_key(), Some("sk-primary".to_string()));
    }

    #[test]
    fn test_record_success() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        let wrapper = FallbackWrapper::new(provider);
        wrapper.record_success();
        let health = wrapper.health_status();
        assert_eq!(health[0].1.consecutive_failures, 0);
    }

    #[test]
    fn test_record_failure_switches_provider() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        let wrapper = FallbackWrapper::new(provider);
        
        assert_eq!(wrapper.current_provider_name(), Some("primary"));
        wrapper.record_failure(true, false); // Rate limit
        assert_eq!(wrapper.current_provider_name(), Some("fallback-1"));
    }

    #[tokio::test]
    async fn test_async_key_resolution() {
        let provider = FallbackApiKeyProvider::new(test_providers());
        let wrapper = FallbackWrapper::new(provider);
        let key = wrapper.current_api_key_async().await;
        assert_eq!(key, Some("sk-primary".to_string()));
    }

    #[test]
    fn test_from_json() {
        let json = r#"{
            "providers": [
                {"name": "a", "api_key": "sk-a"},
                {"name": "b", "api_key": "sk-b"}
            ]
        }"#;
        
        let wrapper = FallbackWrapper::from_json(json).unwrap();
        assert_eq!(wrapper.current_api_key(), Some("sk-a".to_string()));
    }
}
