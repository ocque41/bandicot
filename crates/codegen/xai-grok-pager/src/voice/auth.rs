// Modified in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Bridge the shell's `AuthManager` onto the voice crate's bearer provider.
//!
//! voice-api accepts xAI API keys and OAuth2 tokens directly at `api.x.ai`.
//! This bridge intentionally reads only the xAI `AuthManager`; it never reads
//! the active model's `SamplingConfig` or `OPENAI_API_KEY`. That separation is
//! the provider boundary preventing a custom inference key from being sent to
//! xAI's voice endpoint.
//!
//! Resolved per request: the agent's refreshing manager in direct-spawn mode,
//! or a non-refreshing one that adopts the agent's rotated `auth.json` token
//! under the file lock in leader mode (see [`crate::acp`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use xai_grok_tools::types::SharedApiKeyProvider;
use xai_grok_voice::{SharedVoiceAuth, VoiceAuthProvider};

/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
///
/// Resolves a token per request (never a static snapshot) so a long session
/// follows the underlying `AuthManager` instead of pinning a token that 401s.
struct AuthManagerVoiceAuth(SharedApiKeyProvider);

impl std::fmt::Debug for AuthManagerVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthManagerVoiceAuth")
    }
}

impl VoiceAuthProvider for AuthManagerVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        let provider = self.0.clone();
        Box::pin(async move { provider.current_api_key_async().await })
    }
}

/// Build the voice bearer provider from the connection's `AuthManager`.
///
/// Works for xAI auth managed by `AuthManager`: OAuth / grok.com / OIDC session
/// tokens and xAI API keys. Per-model custom-provider keys are intentionally
/// inaccessible here.
pub fn build_voice_auth(auth_manager: Arc<xai_grok_shell::auth::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth(
        xai_grok_shell::auth::shared_xai_service_api_key_provider(auth_manager),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_test_support::EnvGuard;

    /// An OpenAI key in the process environment is not voice auth. With an
    /// isolated, empty xAI auth store, the provider fails closed with no bearer
    /// instead of forwarding that key to `api.x.ai` or starting login.
    #[tokio::test]
    #[serial_test::serial]
    async fn openai_api_key_is_never_used_for_xai_voice_auth() {
        let _openai = EnvGuard::set("OPENAI_API_KEY", "openai-test-key-must-not-reach-xai");
        let _xai = EnvGuard::unset("XAI_API_KEY");
        let _legacy_xai = EnvGuard::unset("GROK_CODE_XAI_API_KEY");
        let _inline = EnvGuard::unset("GROK_AUTH");
        let auth_home = tempfile::tempdir().expect("temporary xAI auth home");
        let auth_path = auth_home.path().join("missing-auth.json");
        let _path = EnvGuard::set("GROK_AUTH_PATH", &auth_path);
        let auth_manager = Arc::new(xai_grok_shell::auth::AuthManager::new(
            auth_home.path(),
            xai_grok_shell::auth::GrokComConfig::default(),
        ));

        let voice_auth = build_voice_auth(auth_manager);
        assert!(
            voice_auth.bearer().await.is_none(),
            "OPENAI_API_KEY must never be adopted as xAI voice auth",
        );
    }
}
