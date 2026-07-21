//! Acceptance tests for Bandicot's checked-in provider profiles.

use std::path::PathBuf;

use xai_grok_sampler::{ApiBackend, AuthScheme, InferenceTransport};
use xai_grok_shell::agent::config::{Config, resolve_credentials, sampling_config_for_model};
use xai_grok_shell::agent::models::resolve_model_catalog;
use xai_grok_test_support::env::EnvGuard;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("xai-grok-shell must be nested under crates/codegen")
        .to_path_buf()
}

fn load_profile(name: &str) -> Config {
    let source = std::fs::read_to_string(workspace_root().join("config").join(name))
        .unwrap_or_else(|error| panic!("read {name}: {error}"));
    assert!(
        !source.contains("api_key ="),
        "{name} must not contain api_key"
    );
    let raw: toml::Value = toml::from_str(&source).expect("profile is valid TOML");
    let config = Config::new_from_toml_cfg(&raw).expect("profile loads");
    assert!(
        config.model_override_warnings.is_empty(),
        "{name} has model warnings: {:?}",
        config.model_override_warnings
    );
    config
}

#[test]
#[serial_test::serial]
fn cerebras_profile_is_a_deprecated_secret_free_stub_without_models() {
    let _provider_key = EnvGuard::set("CEREBRAS_API_KEY", "cerebras-profile-test-key");
    let _xai_key = EnvGuard::set("XAI_API_KEY", "xai-key-must-not-cross-provider");
    let config = load_profile("cerebras.toml");
    let catalog = resolve_model_catalog(&config, None);
    let cerebras_ids: Vec<_> = catalog
        .keys()
        .filter(|id| id.starts_with("cerebras-"))
        .collect();
    assert!(
        cerebras_ids.is_empty(),
        "deprecated cerebras profile must not expose cerebras models: {cerebras_ids:?}"
    );
}

#[test]
#[serial_test::serial]
fn opencode_go_profile_owns_one_credential_for_every_subscription_model() {
    let _provider_key = EnvGuard::set("OPENCODE_GO_API_KEY", "go-profile-test-key");
    let _xai_key = EnvGuard::set("XAI_API_KEY", "xai-key-must-not-cross-provider");
    let config = load_profile("opencode-go.toml");
    let catalog = resolve_model_catalog(&config, None);

    let expected = [
        "go-kimi-k3",
        "go-kimi-k2.7-code",
        "go-kimi-k2.6",
        "go-kimi-k2.5",
        "go-glm-5.2",
        "go-glm-5.1",
        "go-glm-5",
        "go-minimax-m3",
        "go-minimax-m2.7",
        "go-minimax-m2.5",
        "go-mimo-v2.5-pro",
        "go-mimo-v2.5",
        "go-mimo-v2-pro",
        "go-mimo-v2-omni",
        "go-deepseek-v4-pro",
        "go-deepseek-v4-flash",
        "go-qwen3.7-max",
        "go-qwen3.7-plus",
        "go-qwen3.6-plus",
        "go-qwen3.5-plus",
        "go-grok-4.5",
    ];
    for id in expected {
        let model = catalog
            .get(id)
            .unwrap_or_else(|| panic!("missing OpenCode Go model {id}"));
        assert_eq!(
            model.info.base_url, "https://opencode.ai/zen/go/v1",
            "{id} base URL"
        );
        assert_eq!(
            model.info.api_backend,
            ApiBackend::ChatCompletions,
            "{id} API backend"
        );
        assert!(model.info.capabilities.tools, "{id} tools");
        let credentials = resolve_credentials(model, Some("xai-session-must-not-cross-provider"));
        assert_eq!(
            credentials.api_key.as_deref(),
            Some("go-profile-test-key"),
            "{id} credential owner"
        );
        assert_eq!(
            credentials.auth_scheme,
            AuthScheme::Bearer,
            "{id} auth scheme"
        );
    }
    let go_ids = catalog.keys().filter(|id| id.starts_with("go-")).count();
    assert_eq!(
        go_ids,
        expected.len(),
        "go profile must expose exactly its subscription models"
    );
    assert!(catalog["go-kimi-k3"].info.capabilities.image_input);
    assert!(!catalog["go-deepseek-v4-flash"].info.capabilities.image_input);
}

#[test]
#[serial_test::serial]
fn ollama_profile_is_no_auth_and_does_not_borrow_xai_credentials() {
    let _xai_key = EnvGuard::set("XAI_API_KEY", "xai-key-must-not-cross-provider");
    let config = load_profile("ollama.toml");
    let catalog = resolve_model_catalog(&config, None);

    for id in ["ollama-gpt-oss-20b", "ollama-qwen3-8b"] {
        let model = &catalog[id];
        let credentials = resolve_credentials(model, Some("xai-session-must-not-cross-provider"));
        assert!(credentials.api_key.is_none());
        assert_eq!(credentials.auth_scheme, AuthScheme::None);
        assert!(credentials.base_url.starts_with("http://127.0.0.1:11434/"));
    }
    assert!(
        !catalog["ollama-gpt-oss-20b"]
            .info
            .wire_quirks
            .send_tool_choice
    );
}

#[test]
#[serial_test::serial]
fn apple_profile_is_native_secret_free_and_suppresses_unsupported_capabilities() {
    let _xai_key = EnvGuard::set("XAI_API_KEY", "xai-key-must-not-cross-provider");
    let config = load_profile("apple-foundation-models.toml");
    let catalog = resolve_model_catalog(&config, None);
    let model = &catalog["apple-on-device"];
    let credentials = resolve_credentials(model, Some("session-must-not-cross-provider"));

    assert!(credentials.api_key.is_none());
    assert_eq!(credentials.auth_scheme, AuthScheme::None);
    assert_eq!(
        model.info.transport,
        InferenceTransport::AppleFoundationModels
    );
    assert!(!model.info.capabilities.tools);
    assert!(!model.info.capabilities.image_input);
    assert!(!model.info.supports_backend_search);

    let sampler = sampling_config_for_model(model, credentials, None, None, None, None);
    assert_eq!(sampler.transport, InferenceTransport::AppleFoundationModels);
    assert!(sampler.api_key.is_none());
}
