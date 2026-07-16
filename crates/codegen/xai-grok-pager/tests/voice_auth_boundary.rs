//! Production-surface regression for the xAI voice/OpenAI credential boundary.

use std::sync::Arc;

use xai_grok_test_support::EnvGuard;

/// An OpenAI key in the process environment is not xAI voice auth. With an
/// isolated, empty xAI auth store, the shipped provider must fail closed.
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

    let voice_auth = xai_grok_pager::voice::build_voice_auth(auth_manager);
    assert!(
        voice_auth.bearer().await.is_none(),
        "OPENAI_API_KEY must never be adopted as xAI voice auth",
    );
}
