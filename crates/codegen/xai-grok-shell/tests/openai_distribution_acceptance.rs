//! Acceptance tests for the checked-in, secret-free OpenAI distribution.
//!
//! The profile test is intentionally file-backed: changing
//! `config/openai.toml` changes the test input. The ignored binary test copies
//! that same profile into an isolated `GROK_HOME` and supplies a local Responses
//! API URL through the same Bandicot-owned environment used by the launcher.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use xai_grok_shell::agent::auth_method::{ModelByok, session_token_auth_gate};
use xai_grok_shell::agent::config::{
    Config, TelemetryMode, resolve_credentials, resolve_model_list,
};
use xai_grok_shell::agent::models::resolve_model_catalog;
use xai_grok_shell::config::PromptSuggestModelPin;
use xai_grok_shell::sampling::ApiBackend;
use xai_grok_test_support::env::EnvGuard;
use xai_grok_test_support::{
    MockInferenceServer, MockModelEntry, assert_headless_success, assert_no_crashes, git_workdir,
    run_headless_in_sandbox,
};

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const OPENAI_TEST_KEY: &str = "openai-profile-test-key";
const BANDICOT_BASE_URL_VAR: &str = "BANDICOT_OPENAI_BASE_URL";
const BANDICOT_TOKEN_VAR: &str = "BANDICOT_OPENAI_TOKEN";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("xai-grok-shell must be nested under crates/codegen")
        .to_path_buf()
}

fn profile_path() -> PathBuf {
    workspace_root().join("config/openai.toml")
}

fn read_profile() -> String {
    std::fs::read_to_string(profile_path()).expect("read checked-in config/openai.toml")
}

fn parse_profile(source: &str) -> toml::Value {
    toml::from_str(source).expect("checked-in OpenAI profile must be valid TOML")
}

#[derive(Clone)]
struct SharedLog(Arc<Mutex<Vec<u8>>>);

impl Write for SharedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log mutex poisoned").extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn load_profile_strict(raw: &toml::Value) -> (Config, String) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = {
        let bytes = bytes.clone();
        move || SharedLog(bytes.clone())
    };
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(writer)
        .finish();

    let config = tracing::subscriber::with_default(subscriber, || {
        let config = Config::new_from_toml_cfg(raw).expect("OpenAI profile must load");
        // Resolution also diagnoses deprecated global model settings.
        let _ = resolve_model_list(&config, None);
        config
    });
    let logs = String::from_utf8(bytes.lock().expect("log mutex poisoned").clone())
        .expect("diagnostic log must be UTF-8");
    (config, logs)
}

fn assert_raw_profile_is_secret_free(value: &toml::Value, path: &str) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                assert!(
                    !matches!(
                        key.as_str(),
                        "api_key" | "events_api_key" | "mixpanel_token"
                    ),
                    "secret-bearing field must not be checked in: {child_path}"
                );
                assert_raw_profile_is_secret_free(value, &child_path);
            }
        }
        toml::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                assert_raw_profile_is_secret_free(value, &format!("{path}[{index}]"));
            }
        }
        toml::Value::String(value) => {
            assert!(
                !value.starts_with("sk-"),
                "API-key-shaped secret must not be checked in at {path}"
            );
        }
        _ => {}
    }
}

#[test]
#[serial_test::serial]
fn checked_in_openai_profile_is_complete_secret_free_and_warning_free() {
    // The public loader intentionally applies telemetry env overrides. Exercise
    // the checked-in document itself, independent of the developer/CI shell.
    let _clean_telemetry_env = [
        EnvGuard::unset("GROK_TELEMETRY_ENABLED"),
        EnvGuard::unset("GROK_TELEMETRY_EVENTS_URL"),
        EnvGuard::unset("GROK_TELEMETRY_EVENTS_API_KEY"),
        EnvGuard::unset("GROK_TELEMETRY_MIXPANEL_TOKEN"),
        EnvGuard::unset("GROK_TELEMETRY_MIXPANEL_ENABLED"),
        EnvGuard::unset("GROK_TELEMETRY_TRACE_UPLOAD"),
        EnvGuard::unset("GROK_WEB_SEARCH_MODEL"),
        EnvGuard::unset("GROK_SESSION_SUMMARY_MODEL"),
        EnvGuard::unset("GROK_IMAGE_DESCRIPTION_MODEL"),
        EnvGuard::unset("GROK_PROMPT_SUGGESTIONS_MODEL"),
        EnvGuard::unset("GROK_VOICE_MODE"),
        EnvGuard::unset("OPENAI_API_KEY"),
        EnvGuard::set(BANDICOT_BASE_URL_VAR, OPENAI_BASE_URL),
        EnvGuard::unset(BANDICOT_TOKEN_VAR),
        EnvGuard::set("XAI_API_KEY", "xai-credential-must-not-cross-provider"),
        EnvGuard::set(
            "GROK_CODE_XAI_API_KEY",
            "legacy-xai-credential-must-not-cross-provider",
        ),
    ];
    let source = read_profile();
    let raw = parse_profile(&source);
    assert_raw_profile_is_secret_free(&raw, "");

    let mut expanded = raw.clone();
    xai_grok_shell::config::expand_env_vars_in_toml(&mut expanded);
    let (config, warning_log) = load_profile_strict(&expanded);
    assert!(
        warning_log.trim().is_empty(),
        "checked-in profile must have no unknown, unused, malformed, or deprecated keys:\n{warning_log}"
    );
    assert_eq!(config.cli.auto_update, Some(false));
    assert_eq!(config.models.default.as_deref(), Some("openai-latest"));
    assert_eq!(
        config
            .models
            .default_reasoning_effort
            .map(|effort| effort.as_str()),
        Some("medium")
    );
    assert_eq!(config.models.web_search.as_deref(), Some("openai-luna"));
    assert_eq!(
        config.models.session_summary.as_deref(),
        Some("openai-luna")
    );
    assert_eq!(
        config.models.image_description.as_deref(),
        Some("openai-luna")
    );
    assert_eq!(
        config.models.prompt_suggestion.as_deref(),
        Some("openai-luna")
    );
    assert_eq!(
        config
            .models
            .allowed_models
            .as_deref()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["openai-*", "go-*", "ollama-*", "apple-*"]
    );
    assert_eq!(config.web_search_model, "openai-luna");
    assert_eq!(config.session_summary_model.as_deref(), Some("openai-luna"));
    assert_eq!(
        config.image_description_model.as_deref(),
        Some("openai-luna")
    );
    assert!(matches!(
        config.prompt_suggest_model_pin,
        PromptSuggestModelPin::Pinned(ref model) if model == "openai-luna"
    ));

    assert_eq!(config.features.telemetry, Some(TelemetryMode::Disabled));
    assert_eq!(config.features.feedback, Some(false));
    assert_eq!(config.features.managed_config, Some(false));
    assert_eq!(config.features.remote_fetch, Some(false));
    assert_eq!(config.features.video_gen, Some(false));
    assert_eq!(config.features.voice_mode, Some(false));
    assert!(!config.is_voice_mode_enabled());
    assert_eq!(config.features.backend_tools, Some(false));
    assert!(!config.is_telemetry_enabled());
    assert_eq!(config.telemetry.trace_upload, Some(false));
    assert!(!config.telemetry.mixpanel_enabled);
    assert_eq!(config.telemetry.otel_enabled, Some(false));
    assert_eq!(
        raw.get("features")
            .and_then(|features| features.get("remote_fetch"))
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        raw.get("diagnostics")
            .and_then(|diagnostics| diagnostics.get("error_reporting"))
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(config.diagnostics.error_reporting, Some(false));

    let expected = [
        (
            "openai-latest",
            "gpt-5.6",
            362_000,
            &["none", "low", "medium", "high", "xhigh", "max"][..],
        ),
        (
            "openai-sol",
            "gpt-5.6-sol",
            362_000,
            &["none", "low", "medium", "high", "xhigh", "max"][..],
        ),
        (
            "openai-terra",
            "gpt-5.6-terra",
            362_000,
            &["none", "low", "medium", "high", "xhigh", "max"][..],
        ),
        (
            "openai-luna",
            "gpt-5.6-luna",
            362_000,
            &["none", "low", "medium", "high", "xhigh", "max"][..],
        ),
        (
            "openai-codex",
            "gpt-5.3-codex",
            362_000,
            &["low", "medium", "high", "xhigh"][..],
        ),
    ];
    let additional_selectable = [
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
        "ollama-gpt-oss-20b",
        "ollama-qwen3-8b",
        "apple-on-device",
    ];
    assert_eq!(
        config.config_models.len(),
        expected.len() + additional_selectable.len()
    );

    let catalog = resolve_model_catalog(&config, None);
    let mut selectable: Vec<_> = catalog
        .iter()
        .filter(|(_, entry)| entry.info.user_selectable)
        .map(|(key, _)| key.as_str())
        .collect();
    let mut expected_selectable: Vec<_> = expected.iter().map(|(key, ..)| *key).collect();
    expected_selectable.extend(additional_selectable);
    selectable.sort_unstable();
    expected_selectable.sort_unstable();
    assert_eq!(
        selectable, expected_selectable,
        "the picker must not expose compiled xAI defaults"
    );
    assert_eq!(
        catalog["openai-latest"]
            .info
            .reasoning_effort
            .map(|effort| effort.as_str()),
        Some("medium"),
        "profile default reasoning effort must reach the selected model"
    );

    for &(key, model_id, context_window, expected_efforts) in &expected {
        let entry = catalog
            .get(key)
            .unwrap_or_else(|| panic!("missing checked-in profile entry {key}"));
        assert_eq!(entry.info.model, model_id, "{key} routing slug");
        assert_eq!(entry.info.base_url, OPENAI_BASE_URL, "{key} base URL");
        assert_eq!(entry.info.api_backend, ApiBackend::Responses, "{key} API");
        assert_eq!(entry.info.agent_type, "codex", "{key} agent type");
        assert_eq!(
            entry.info.context_window.get(),
            context_window,
            "{key} context window"
        );
        assert_eq!(
            entry.info.max_completion_tokens,
            Some(128_000),
            "{key} max output"
        );
        assert_eq!(entry.info.temperature, None, "{key} temperature");
        assert_eq!(entry.info.top_p, None, "{key} top_p");
        assert_eq!(
            config.config_models[key].auto_compact_threshold_percent,
            Some(51),
            "{key} compact threshold"
        );
        assert!(!entry.info.supports_backend_search, "{key} hosted search");
        assert!(entry.info.supports_reasoning_effort, "{key} reasoning gate");
        assert!(entry.api_key.is_none(), "{key} must not contain an API key");
        assert_eq!(
            entry.env_key.as_ref().map(ToString::to_string).as_deref(),
            Some(BANDICOT_TOKEN_VAR),
            "{key} credential environment"
        );
        assert!(
            entry.info.extra_headers.keys().all(|name| {
                !name.eq_ignore_ascii_case("authorization")
                    && !name.eq_ignore_ascii_case("x-api-key")
            }),
            "{key} must not check in credential headers"
        );
        let efforts: Vec<_> = entry
            .info
            .reasoning_efforts
            .iter()
            .map(|option| option.value.as_str())
            .collect();
        assert_eq!(efforts, expected_efforts, "{key} reasoning menu");

        let missing_provider_key = resolve_credentials(
            entry,
            Some("xai-session-credential-must-not-cross-provider"),
        );
        assert!(
            missing_provider_key.api_key.is_none(),
            "{key} must fail closed instead of borrowing any xAI credential"
        );
        assert_eq!(
            missing_provider_key.auth_type,
            xai_chat_state::AuthType::ApiKey,
            "{key} must never become a refreshable xAI session route"
        );
    }

    let mut undeclared_custom_route = catalog["openai-latest"].clone();
    undeclared_custom_route.env_key = None;
    undeclared_custom_route.api_key = None;
    let custom_without_key = resolve_credentials(
        &undeclared_custom_route,
        Some("xai-session-credential-must-not-cross-provider"),
    );
    assert!(
        custom_without_key.api_key.is_none(),
        "a non-xAI endpoint remains provider-isolated even without env_key"
    );
    assert!(
        !session_token_auth_gate(true, ModelByok::NotByok, false),
        "session refresh must always require a verified first-party endpoint"
    );

    {
        let _openai_key = EnvGuard::set(BANDICOT_TOKEN_VAR, "openai-provider-test-key");
        let own_provider_key = resolve_credentials(
            &catalog["openai-latest"],
            Some("xai-session-credential-must-not-cross-provider"),
        );
        assert_eq!(
            own_provider_key.api_key.as_deref(),
            Some("openai-provider-test-key")
        );
        assert_eq!(own_provider_key.base_url, OPENAI_BASE_URL);
        assert_eq!(own_provider_key.auth_type, xai_chat_state::AuthType::ApiKey);
    }
}

fn write_mock_profile(grok_home: &Path, mock_url: &str) {
    let source = read_profile();
    assert_eq!(
        source.matches("${BANDICOT_OPENAI_BASE_URL}").count(),
        5,
        "every curated model must use the launcher-selected provider URL"
    );
    assert!(mock_url.starts_with("http://127.0.0.1:"));
    std::fs::create_dir_all(grok_home).expect("create isolated GROK_HOME");
    std::fs::write(grok_home.join("config.toml"), source)
        .expect("write mock-routed OpenAI profile");
}

#[tokio::test]
#[ignore = "requires GROK_BINARY pointing to a pre-built xai-grok-pager binary"]
async fn built_binary_uses_checked_in_openai_profile_end_to_end() {
    let binary_from_env = std::env::var_os("GROK_BINARY")
        .map(PathBuf::from)
        .expect("set GROK_BINARY to the pre-built xai-grok-pager artifact");
    let binary = std::fs::canonicalize(&binary_from_env).unwrap_or_else(|error| {
        panic!(
            "cannot resolve GROK_BINARY {}: {error}",
            binary_from_env.display()
        )
    });
    assert!(
        binary.is_file(),
        "GROK_BINARY is not a file: {}",
        binary.display()
    );

    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::with_agent_type("gpt-5.6", "codex").with_api_backend("responses")],
        OPENAI_TEST_KEY,
    )
    .await
    .expect("start authenticated Responses API mock");
    server.set_response("OPENAI_PROFILE_OK");

    let mut workdir = git_workdir();
    write_mock_profile(workdir.grok_home(), &server.url());
    workdir
        .set_env(BANDICOT_BASE_URL_VAR, server.url())
        .set_env(BANDICOT_TOKEN_VAR, OPENAI_TEST_KEY)
        .set_env("GROK_TELEMETRY_ENABLED", "false")
        .set_env("GROK_FEEDBACK_ENABLED", "false")
        .set_env("GROK_TRACE_UPLOAD", "false")
        .set_env("GROK_INSTRUMENTATION", "disabled")
        .set_env("GROK_DISABLE_AUTOUPDATER", "1")
        .set_env("GROK_VOICE_MODE", "0")
        .set_env("GROK_IMAGE_GEN", "0")
        .set_env("GROK_IMAGE_EDIT", "0")
        .set_env("GROK_VIDEO_GEN", "0");

    let mut command = tokio::process::Command::new(&binary);
    command
        .args([
            "-p",
            "Reply with exactly OPENAI_PROFILE_OK",
            "--yolo",
            "--model",
            "openai-latest",
            "--max-turns",
            "1",
        ])
        .current_dir(workdir.workspace())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let result = run_headless_in_sandbox(command, workdir).await;
    assert_headless_success(&result, "checked-in OpenAI profile", Some(&server));
    assert_no_crashes(&result.stderr);
    assert_eq!(result.stdout.trim(), "OPENAI_PROFILE_OK");
    assert!(
        !result.stderr.to_ascii_lowercase().contains("login"),
        "OpenAI API-key flow must not ask for xAI login:\n{}",
        result.stderr
    );
    assert!(
        !result.stderr.to_ascii_lowercase().contains("authenticate"),
        "OpenAI API-key flow must not ask for browser authentication:\n{}",
        result.stderr
    );

    assert!(
        server.has_responses_request(),
        "profile must use /v1/responses:\n{}",
        server.request_log_summary()
    );
    assert!(
        !server.has_chat_completion_request(),
        "profile must never fall back to Chat Completions:\n{}",
        server.request_log_summary()
    );

    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.path == "/v1/responses"),
        "remote xAI model/settings/user surfaces must remain disabled:\n{}",
        server.request_log_summary()
    );
    for request in &requests {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer openai-profile-test-key"),
            "every inference request must use only BANDICOT_OPENAI_TOKEN"
        );
        assert!(
            request.headers.iter().all(|(name, _)| {
                !name.starts_with("x-grok-")
                    && !name.starts_with("x-xai-")
                    && name != "x-api-key"
                    && name != "x-compactions-remaining"
                    && name != "x-compaction-at"
            }),
            "OpenAI requests must not carry xAI-only headers: {:?}",
            request.headers
        );
    }

    let main_request = requests
        .iter()
        .filter_map(|request| request.body.as_ref())
        .find(|body| body.get("model").and_then(serde_json::Value::as_str) == Some("gpt-5.6"))
        .expect("main request must route openai-latest to gpt-5.6");
    assert_eq!(
        main_request
            .pointer("/reasoning/effort")
            .and_then(serde_json::Value::as_str),
        Some("medium")
    );
    assert!(
        main_request
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|tools| {
                tools.iter().all(|tool| {
                    let name = tool
                        .get("name")
                        .or_else(|| tool.pointer("/function/name"))
                        .or_else(|| tool.get("type"))
                        .and_then(serde_json::Value::as_str);
                    name != Some("x_search")
                })
            }),
        "OpenAI profile must not inject x_search: {main_request}"
    );
}
