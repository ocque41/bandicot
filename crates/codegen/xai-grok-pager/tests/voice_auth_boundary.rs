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

/// A generic external provider can authenticate model traffic, but its bearer
/// is not valid for xAI-only voice services unless it explicitly declares the
/// first-party xAI issuer.
#[tokio::test]
#[serial_test::serial]
async fn third_party_external_bearer_is_never_used_for_xai_voice_auth() {
    let _openai = EnvGuard::unset("OPENAI_API_KEY");
    let _xai = EnvGuard::unset("XAI_API_KEY");
    let _legacy_xai = EnvGuard::unset("GROK_CODE_XAI_API_KEY");
    let _inline = EnvGuard::set(
        "GROK_AUTH",
        r#"{"key":"third-party-external-bearer","auth_mode":"external","create_time":"2026-07-16T00:00:00Z","user_id":"external-user","email":null,"oidc_issuer":"https://idp.example"}"#,
    );
    let auth_home = tempfile::tempdir().expect("temporary xAI auth home");
    let auth_manager = Arc::new(xai_grok_shell::auth::AuthManager::new(
        auth_home.path(),
        xai_grok_shell::auth::GrokComConfig::default(),
    ));

    let voice_auth = xai_grok_pager::voice::build_voice_auth(auth_manager);
    assert!(
        voice_auth.bearer().await.is_none(),
        "third-party external bearer must never be forwarded to xAI voice",
    );
}

/// Positive boundary: an external credential explicitly issued by xAI remains
/// valid for xAI voice and is reclassified on every request.
#[tokio::test]
#[serial_test::serial]
async fn xai_external_bearer_remains_available_for_xai_voice_auth() {
    let _openai = EnvGuard::unset("OPENAI_API_KEY");
    let _xai = EnvGuard::unset("XAI_API_KEY");
    let _legacy_xai = EnvGuard::unset("GROK_CODE_XAI_API_KEY");
    let inline = format!(
        r#"{{"key":"xai-external-bearer","auth_mode":"external","create_time":"2026-07-16T00:00:00Z","user_id":"external-user","email":null,"oidc_issuer":"{}"}}"#,
        xai_grok_shell::auth::XAI_OAUTH2_ISSUER,
    );
    let _inline = EnvGuard::set("GROK_AUTH", inline);
    let auth_home = tempfile::tempdir().expect("temporary xAI auth home");
    let auth_manager = Arc::new(xai_grok_shell::auth::AuthManager::new(
        auth_home.path(),
        xai_grok_shell::auth::GrokComConfig::default(),
    ));

    let voice_auth = xai_grok_pager::voice::build_voice_auth(auth_manager);
    assert_eq!(
        voice_auth.bearer().await.as_deref(),
        Some("xai-external-bearer"),
    );
}
