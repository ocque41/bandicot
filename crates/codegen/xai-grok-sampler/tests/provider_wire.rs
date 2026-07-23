// Added in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Provider-boundary wire acceptance tests using a real HTTP listener.

use indexmap::IndexMap;
use std::time::Duration;

use xai_grok_sampler::{
    ApiBackend, AuthScheme, ChatMaxTokensField, ProviderCapabilities, ReasoningResponseField,
    RequestId, ResolvedServiceTier, SamplerConfig, SamplingClient, ServiceTierCapabilities,
    ServiceTierPreference, ServiceTierSource, WireQuirks, collect_response,
    stream_chat_completions, stream_responses,
};
use xai_grok_sampling_types::{
    ConversationItem, ConversationRequest, ConversationResponse, ConversationToolChoice,
    DoomLoopRecoveryPolicy, HostedMultiAgentCapability, HostedMultiAgentConfig, HostedTool,
    OPENAI_RESPONSES_MULTI_AGENT_BETA, ReasoningEffort, SamplingError, ToolSpec,
    resolve_service_tier,
};
use xai_grok_test_support::{MockInferenceServer, MockModelEntry, ScriptedResponse, SseEvent};

async fn sample_chat(
    client: &SamplingClient,
    request: ConversationRequest,
) -> ConversationResponse {
    let (chunks, metadata) = client
        .conversation_stream(request)
        .await
        .expect("mock accepts Chat Completions request");
    collect_response(stream_chat_completions(
        chunks,
        metadata,
        RequestId::from("provider-wire"),
        Duration::from_secs(5),
    ))
    .await
    .expect("stream completes")
    .0
}

fn provider_wire_responses_config(
    base_url: String,
    effective_service_tier: ResolvedServiceTier,
    priority_supported: bool,
) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("openai-test-key".to_owned()),
        base_url,
        model: "gpt-test".to_owned(),
        max_completion_tokens: Some(2048),
        temperature: Some(0.2),
        top_p: Some(0.9),
        api_backend: ApiBackend::Responses,
        auth_scheme: AuthScheme::Bearer,
        capabilities: ProviderCapabilities {
            service_tiers: ServiceTierCapabilities {
                priority: priority_supported,
                default_service_tier: None,
            },
            ..ProviderCapabilities::default()
        },
        effective_service_tier,
        reasoning_effort: Some(ReasoningEffort::High),
        ..SamplerConfig::default()
    }
}

fn rich_responses_request() -> ConversationRequest {
    let mut request = ConversationRequest::from_items(vec![ConversationItem::user("hello")]);
    request.tools.push(ToolSpec {
        name: "lookup".to_owned(),
        description: Some("Look up provider-neutral context".to_owned()),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"],
        }),
    });
    request.tool_choice = Some(ConversationToolChoice::Auto);
    request.max_output_tokens = Some(1234);
    request.temperature = Some(0.4);
    request.top_p = Some(0.8);
    request.reasoning_effort = Some(ReasoningEffort::High);
    request
}

fn captured_responses_bodies(server: &MockInferenceServer) -> Vec<serde_json::Value> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/v1/responses")
        .map(|request| request.body.expect("responses JSON body captured"))
        .collect()
}

fn without_service_tier(mut value: serde_json::Value) -> serde_json::Value {
    value
        .as_object_mut()
        .expect("request body is an object")
        .remove("service_tier");
    value
}

fn fast_responses_config(base_url: String) -> SamplerConfig {
    provider_wire_responses_config(
        base_url,
        resolve_service_tier(
            ServiceTierPreference::Fast,
            &ServiceTierCapabilities {
                priority: true,
                default_service_tier: None,
            },
            ServiceTierSource::Session,
        ),
        true,
    )
}

fn service_tier_rejection_response() -> ScriptedResponse {
    ScriptedResponse::json(
        400,
        serde_json::json!({
            "error": {
                "message": "Unsupported value: 'priority'.",
                "type": "invalid_request_error",
                "param": "service_tier",
                "code": "unsupported_value"
            }
        }),
    )
}

fn generic_validation_error_response() -> ScriptedResponse {
    ScriptedResponse::json(
        400,
        serde_json::json!({
            "error": {
                "message": "Invalid model.",
                "type": "invalid_request_error",
                "param": "model",
                "code": "invalid_value"
            }
        }),
    )
}

fn minimal_responses_success(text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "resp_test",
        "object": "response",
        "created_at": 1234567890,
        "model": "gpt-test",
        "status": "completed",
        "output": [{
            "type": "message",
            "id": "msg_test",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": text,
                "annotations": []
            }]
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "total_tokens": 15,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens_details": { "reasoning_tokens": 0 }
        }
    })
}

fn provider_wire_responses_multi_agent_config(
    base_url: String,
    supported: bool,
    enabled: bool,
    provider_max: Option<u32>,
    requested_max: Option<u32>,
) -> SamplerConfig {
    let mut config =
        provider_wire_responses_config(base_url, ResolvedServiceTier::default(), false);
    config.capabilities.hosted_multi_agent = HostedMultiAgentCapability {
        supported,
        max_concurrent_subagents: provider_max,
    };
    config.hosted_multi_agent = HostedMultiAgentConfig {
        enabled,
        max_concurrent_subagents: requested_max,
    };
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_service_tier_is_sent_for_non_streaming_and_streaming() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let config = provider_wire_responses_config(
        server.url(),
        resolve_service_tier(
            ServiceTierPreference::Fast,
            &ServiceTierCapabilities {
                priority: true,
                default_service_tier: None,
            },
            ServiceTierSource::Session,
        ),
        true,
    );
    let client = SamplingClient::new(config).expect("sampling client builds");

    let _ = client
        .conversation_responses(rich_responses_request())
        .await;
    let (_stream, _metadata, _doom_loop) = client
        .conversation_stream_responses(rich_responses_request())
        .await
        .expect("mock accepts streaming Responses request");

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["service_tier"], "priority");
    assert_eq!(bodies[1]["service_tier"], "priority");
    assert_eq!(
        bodies[0].get("stream").and_then(|value| value.as_bool()),
        None
    );
    assert_eq!(
        bodies[1].get("stream").and_then(|value| value.as_bool()),
        Some(true)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_standard_and_unsupported_fast_omit_priority_tier() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let capabilities = ServiceTierCapabilities {
        priority: true,
        default_service_tier: None,
    };
    let standard_client = SamplingClient::new(provider_wire_responses_config(
        server.url(),
        resolve_service_tier(
            ServiceTierPreference::Standard,
            &capabilities,
            ServiceTierSource::Session,
        ),
        true,
    ))
    .expect("standard client builds");
    let unsupported = resolve_service_tier(
        ServiceTierPreference::Fast,
        &ServiceTierCapabilities::default(),
        ServiceTierSource::Session,
    );
    assert_eq!(unsupported.requested, ServiceTierPreference::Fast);
    assert!(!unsupported.supported);
    let unsupported_client = SamplingClient::new(provider_wire_responses_config(
        server.url(),
        unsupported,
        false,
    ))
    .expect("unsupported client builds");

    let _ = standard_client
        .conversation_stream_responses(rich_responses_request())
        .await;
    let _ = unsupported_client
        .conversation_stream_responses(rich_responses_request())
        .await;

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].get("service_tier").is_none());
    assert!(bodies[1].get("service_tier").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_only_changes_service_tier_field() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let capabilities = ServiceTierCapabilities {
        priority: true,
        default_service_tier: None,
    };
    let standard_client = SamplingClient::new(provider_wire_responses_config(
        server.url(),
        resolve_service_tier(
            ServiceTierPreference::Standard,
            &capabilities,
            ServiceTierSource::Session,
        ),
        true,
    ))
    .expect("standard client builds");
    let fast_client = SamplingClient::new(provider_wire_responses_config(
        server.url(),
        resolve_service_tier(
            ServiceTierPreference::Fast,
            &capabilities,
            ServiceTierSource::Session,
        ),
        true,
    ))
    .expect("fast client builds");

    let _ = standard_client
        .conversation_stream_responses(rich_responses_request())
        .await;
    let _ = fast_client
        .conversation_stream_responses(rich_responses_request())
        .await;

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].get("service_tier").is_none());
    assert_eq!(bodies[1]["service_tier"], "priority");
    assert_eq!(
        bodies[0].pointer("/reasoning/effort"),
        bodies[1].pointer("/reasoning/effort")
    );
    assert_eq!(
        without_service_tier(bodies[0].clone()),
        without_service_tier(bodies[1].clone())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_retries_once_without_service_tier_on_typed_rejection() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response("/v1/responses", service_tier_rejection_response());
    server.enqueue_response(
        "/v1/responses",
        ScriptedResponse::json(200, minimal_responses_success("STANDARD_OK")),
    );
    let client =
        SamplingClient::new(fast_responses_config(server.url())).expect("fast client builds");

    client
        .conversation_responses(rich_responses_request())
        .await
        .expect("typed service_tier rejection falls back to standard request");

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["service_tier"], "priority");
    assert!(bodies[1].get("service_tier").is_none());
    assert_eq!(without_service_tier(bodies[0].clone()), bodies[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_stream_retries_once_without_service_tier_on_typed_rejection() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response("/v1/responses", service_tier_rejection_response());
    server.enqueue_response(
        "/v1/responses",
        ScriptedResponse::sse(
            xai_grok_test_support::sse::responses_api_reasoning_and_text_events(
                "fallback",
                "STANDARD_STREAM_OK",
                "gpt-test",
            ),
        ),
    );
    let client =
        SamplingClient::new(fast_responses_config(server.url())).expect("fast client builds");

    let (raw, metadata, doom_loop) = client
        .conversation_stream_responses(rich_responses_request())
        .await
        .expect("typed service_tier rejection falls back to standard stream");
    let (response, _metrics) = collect_response(stream_responses(
        raw,
        metadata,
        RequestId::from("provider-wire"),
        Duration::from_secs(5),
        doom_loop,
    ))
    .await
    .expect("fallback stream completes");
    assert_eq!(
        response
            .assistant()
            .expect("assistant response")
            .content
            .as_ref(),
        "STANDARD_STREAM_OK"
    );

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["service_tier"], "priority");
    assert!(bodies[1].get("service_tier").is_none());
    assert_eq!(without_service_tier(bodies[0].clone()), bodies[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_does_not_retry_generic_validation_errors() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response("/v1/responses", generic_validation_error_response());
    server.enqueue_response(
        "/v1/responses",
        ScriptedResponse::json(200, minimal_responses_success("UNUSED")),
    );
    let client =
        SamplingClient::new(fast_responses_config(server.url())).expect("fast client builds");

    let err = client
        .conversation_responses(rich_responses_request())
        .await
        .expect_err("generic validation errors are not retried");
    assert!(matches!(
        err,
        SamplingError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            ..
        }
    ));

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["service_tier"], "priority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_fast_does_not_retry_auth_errors() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response(
        "/v1/responses",
        ScriptedResponse::json(
            401,
            serde_json::json!({
                "error": {
                    "message": "Unauthorized",
                    "type": "invalid_request_error",
                    "param": "service_tier",
                    "code": "unauthorized"
                }
            }),
        ),
    );
    server.enqueue_response(
        "/v1/responses",
        ScriptedResponse::json(200, minimal_responses_success("UNUSED")),
    );
    let client =
        SamplingClient::new(fast_responses_config(server.url())).expect("fast client builds");

    let err = client
        .conversation_responses(rich_responses_request())
        .await
        .expect_err("401 errors are not retried");
    assert!(matches!(err, SamplingError::Auth(_)));

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["service_tier"], "priority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_hosted_multi_agent_enable_sends_beta_header_and_body() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let client = SamplingClient::new(provider_wire_responses_multi_agent_config(
        server.url(),
        true,
        true,
        Some(4),
        Some(8),
    ))
    .expect("hosted multi-agent client builds");

    let _ = client
        .conversation_responses(rich_responses_request())
        .await;

    let requests = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/v1/responses")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].header("openai-beta"),
        Some(OPENAI_RESPONSES_MULTI_AGENT_BETA)
    );
    let body = requests[0]
        .body
        .as_ref()
        .expect("responses JSON body captured");
    assert_eq!(body["multi_agent"]["enabled"], true);
    assert_eq!(body["multi_agent"]["max_concurrent_subagents"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_hosted_multi_agent_is_omitted_without_support_or_enable() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let unsupported_client = SamplingClient::new(provider_wire_responses_multi_agent_config(
        server.url(),
        false,
        true,
        None,
        Some(3),
    ))
    .expect("unsupported hosted multi-agent client builds");
    let disabled_client = SamplingClient::new(provider_wire_responses_multi_agent_config(
        server.url(),
        true,
        false,
        Some(3),
        Some(3),
    ))
    .expect("disabled hosted multi-agent client builds");

    let _ = unsupported_client
        .conversation_responses(rich_responses_request())
        .await;
    let _ = disabled_client
        .conversation_responses(rich_responses_request())
        .await;

    let requests = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/v1/responses")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(request.header("openai-beta").is_none());
        let body = request.body.as_ref().expect("responses JSON body captured");
        assert!(body.get("multi_agent").is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_prompt_cache_key_is_capability_gated_and_content_free() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");
    let mut supported = fast_responses_config(server.url());
    supported.capabilities.prompt_cache = true;
    let supported = SamplingClient::new(supported).expect("cache client builds");
    let unsupported =
        SamplingClient::new(fast_responses_config(server.url())).expect("non-cache client builds");

    let _ = supported
        .conversation_responses(rich_responses_request())
        .await;
    let _ = unsupported
        .conversation_responses(rich_responses_request())
        .await;

    let bodies = captured_responses_bodies(&server);
    assert_eq!(bodies.len(), 2);
    let key = bodies[0]["prompt_cache_key"]
        .as_str()
        .expect("cache key emitted");
    assert!(key.starts_with("bandicot-v1-"));
    assert!(!key.contains("Review the repository"));
    assert!(bodies[1].get("prompt_cache_key").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_compatible_responses_request_has_no_xai_wire_extensions() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-test")],
        "openai-test-key",
    )
    .await
    .expect("mock server starts");

    let mut extra_headers = IndexMap::new();
    extra_headers.insert("x-grok-extra".to_owned(), "must-not-leak".to_owned());
    extra_headers.insert("x-xai-token-auth".to_owned(), "must-not-leak".to_owned());
    extra_headers.insert(
        "x-compactions-remaining".to_owned(),
        "must-not-leak".to_owned(),
    );
    extra_headers.insert("x-compaction-at".to_owned(), "must-not-leak".to_owned());
    let config = SamplerConfig {
        api_key: Some("openai-test-key".to_owned()),
        base_url: server.url(),
        model: "gpt-test".to_owned(),
        api_backend: ApiBackend::Responses,
        auth_scheme: AuthScheme::Bearer,
        extra_headers,
        stream_tool_calls: true,
        doom_loop_recovery: Some(DoomLoopRecoveryPolicy::default()),
        client_identifier: Some("must-not-leak".to_owned()),
        client_version: Some("must-not-leak".to_owned()),
        deployment_id: Some("must-not-leak".to_owned()),
        user_id: Some("must-not-leak".to_owned()),
        ..SamplerConfig::default()
    };
    let client = SamplingClient::new(config).expect("sampling client builds");

    let mut request = ConversationRequest::from_items(vec![ConversationItem::user("hello")]);
    request.hosted_tools = vec![
        HostedTool::WebSearch { options: None },
        HostedTool::XSearch { options: None },
    ];
    request.tools.push(ToolSpec {
        name: "x_search".to_owned(),
        description: Some("provider-neutral function".to_owned()),
        parameters: serde_json::json!({"type": "object"}),
    });
    request.x_grok_conv_id = Some("conv-private".to_owned());
    request.x_grok_req_id = Some("req-private".to_owned());
    request.x_grok_session_id = Some("session-private".to_owned());
    request.x_grok_agent_id = Some("agent-private".to_owned());

    let (_events, _metadata, doom_loop) = client
        .conversation_stream_responses(request)
        .await
        .expect("mock accepts OpenAI-compatible request");
    assert!(
        doom_loop.is_none(),
        "custom/OpenAI hosts cannot opt into xAI doom checks"
    );

    let entries = server.requests();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "/v1/responses")
        .expect("one Responses request captured");
    assert_eq!(
        entry.authorization.as_deref(),
        Some("Bearer openai-test-key")
    );
    assert_eq!(entry.header("content-type"), Some("application/json"));
    assert!(entry.header("user-agent").is_some());
    assert!(
        entry.headers.iter().all(|(name, _)| {
            !name.starts_with("x-grok-")
                && !name.starts_with("x-xai-")
                && name != "x-compactions-remaining"
                && name != "x-compaction-at"
        }),
        "captured headers must not contain xAI-only identity/tracking fields: {:?}",
        entry.headers
    );

    let body = entry.body.as_ref().expect("JSON request body captured");
    assert_eq!(
        body.get("stream").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(body.get("stream_tool_calls").is_none());
    let tool_types: Vec<&str> = body
        .get("tools")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("type").and_then(|value| value.as_str()))
        .collect();
    assert!(tool_types.contains(&"web_search"));
    assert!(!tool_types.contains(&"x_search"));
    assert!(body["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool.get("type").and_then(serde_json::Value::as_str) == Some("function")
                && tool.get("name").and_then(serde_json::Value::as_str) == Some("x_search")
        })
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_loopback_no_auth_streams_text_without_proxy_headers() {
    let server = MockInferenceServer::start()
        .await
        .expect("mock server starts");
    server.set_response("LOCAL_STREAM_OK");
    let client = SamplingClient::new(SamplerConfig {
        api_key: Some("credential-must-not-be-sent".to_owned()),
        base_url: server.url(),
        model: "local-model".to_owned(),
        auth_scheme: AuthScheme::None,
        extra_headers: [
            ("x-xai-token-auth".to_owned(), "must-not-leak".to_owned()),
            (
                "x-grok-client-version".to_owned(),
                "must-not-leak".to_owned(),
            ),
        ]
        .into_iter()
        .collect(),
        ..SamplerConfig::default()
    })
    .expect("local client builds");

    let response = sample_chat(
        &client,
        ConversationRequest::from_items(vec![ConversationItem::user("hello")]),
    )
    .await;
    assert_eq!(
        response
            .assistant()
            .expect("assistant response")
            .content
            .as_ref(),
        "LOCAL_STREAM_OK"
    );

    let request = server
        .requests()
        .into_iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request captured");
    assert!(request.authorization.is_none());
    assert!(request.header("x-api-key").is_none());
    assert!(
        request
            .headers
            .iter()
            .all(|(name, _)| { !name.starts_with("x-grok-") && !name.starts_with("x-xai-") })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cerebras_chat_uses_declared_wire_quirks_and_streams_text() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("gpt-oss-120b")],
        "cerebras-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response(
        "/v1/chat/completions",
        ScriptedResponse::sse(vec![
            SseEvent::data(
                serde_json::json!({
                    "id":"chatcmpl-cerebras", "object":"chat.completion.chunk",
                    "created":1, "model":"gpt-oss-120b",
                    "choices":[{"index":0,"delta":{"reasoning":"check the request"},"finish_reason":null}]
                })
                .to_string(),
            ),
            SseEvent::data(
                serde_json::json!({
                    "id":"chatcmpl-cerebras", "object":"chat.completion.chunk",
                    "created":1, "model":"gpt-oss-120b",
                    "choices":[{"index":0,"delta":{"content":"CEREBRAS_STREAM_OK"},"finish_reason":null}]
                })
                .to_string(),
            ),
            SseEvent::data(
                serde_json::json!({
                    "id":"chatcmpl-cerebras", "object":"chat.completion.chunk",
                    "created":1, "model":"gpt-oss-120b",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]
                })
                .to_string(),
            ),
            SseEvent::data("[DONE]"),
        ]),
    );
    let client = SamplingClient::new(SamplerConfig {
        api_key: Some("cerebras-test-key".to_owned()),
        base_url: server.url(),
        model: "gpt-oss-120b".to_owned(),
        max_completion_tokens: Some(4096),
        auth_scheme: AuthScheme::Bearer,
        capabilities: ProviderCapabilities {
            tools: true,
            image_input: false,
            ..ProviderCapabilities::default()
        },
        wire_quirks: WireQuirks {
            chat_max_tokens_field: ChatMaxTokensField::MaxCompletionTokens,
            reasoning_response_field: ReasoningResponseField::Reasoning,
            send_stream_options: false,
            send_tool_choice: true,
        },
        ..SamplerConfig::default()
    })
    .expect("Cerebras client builds");

    let response = sample_chat(
        &client,
        ConversationRequest::from_items(vec![ConversationItem::user("hello")]),
    )
    .await;
    assert_eq!(
        response
            .assistant()
            .expect("assistant response")
            .content
            .as_ref(),
        "CEREBRAS_STREAM_OK"
    );
    assert!(response.reasoning_items().next().is_some());

    let request = server
        .requests()
        .into_iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request captured");
    assert_eq!(
        request.authorization.as_deref(),
        Some("Bearer cerebras-test-key")
    );
    let body = request.body.expect("request body captured");
    assert_eq!(body["max_completion_tokens"], 4096);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("stream_options").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ollama_no_auth_tool_call_result_continuation_omits_tool_choice() {
    let server = MockInferenceServer::start()
        .await
        .expect("mock server starts");
    server.enqueue_response(
        "/v1/chat/completions",
        ScriptedResponse::sse(
            xai_grok_test_support::sse::chat_completions_reasoning_then_tool_call_events(
                "inspect the workspace",
                "call_1",
                "read_file",
                r#"{"path":"README.md"}"#,
                "gpt-oss:20b",
            ),
        ),
    );
    server.set_response("TOOL_RESULT_ACCEPTED");
    let client = SamplingClient::new(SamplerConfig {
        base_url: server.url(),
        model: "gpt-oss:20b".to_owned(),
        auth_scheme: AuthScheme::None,
        capabilities: ProviderCapabilities {
            tools: true,
            image_input: false,
            ..ProviderCapabilities::default()
        },
        wire_quirks: WireQuirks {
            send_tool_choice: false,
            ..WireQuirks::default()
        },
        ..SamplerConfig::default()
    })
    .expect("Ollama client builds");
    let tool = ToolSpec {
        name: "read_file".to_owned(),
        description: Some("Read a file".to_owned()),
        parameters: serde_json::json!({"type":"object"}),
    };
    let mut first = ConversationRequest::from_items(vec![ConversationItem::user("read README")]);
    first.tools = vec![tool.clone()];
    first.tool_choice = Some(ConversationToolChoice::Auto);
    let first_response = sample_chat(&client, first).await;
    let assistant = first_response
        .items
        .iter()
        .find_map(|item| match item {
            ConversationItem::Assistant(assistant) if !assistant.tool_calls.is_empty() => {
                Some(item.clone())
            }
            _ => None,
        })
        .expect("tool call parsed from stream");

    let mut second = ConversationRequest::from_items(vec![
        ConversationItem::user("read README"),
        assistant,
        ConversationItem::tool_result("call_1", "Bandicot README contents"),
    ]);
    second.tools = vec![tool];
    second.tool_choice = Some(ConversationToolChoice::Auto);
    let second_response = sample_chat(&client, second).await;
    assert_eq!(
        second_response
            .assistant()
            .expect("assistant response")
            .content
            .as_ref(),
        "TOOL_RESULT_ACCEPTED"
    );

    let requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/v1/chat/completions")
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.is_none())
    );
    assert!(requests.iter().all(|request| {
        request
            .body
            .as_ref()
            .is_some_and(|body| body.get("tool_choice").is_none())
    }));
    let continuation = requests[1].body.as_ref().expect("continuation body");
    assert!(continuation["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "tool"
                && message["tool_call_id"] == "call_1"
                && message["content"] == "Bandicot README contents"
        })
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opencode_go_cost_extension_events_do_not_break_the_stream() {
    let server = MockInferenceServer::start_with_required_auth(
        vec![MockModelEntry::new("deepseek-v4-flash")],
        "go-test-key",
    )
    .await
    .expect("mock server starts");
    server.enqueue_response(
        "/v1/chat/completions",
        ScriptedResponse::sse(vec![
            SseEvent::data(
                serde_json::json!({
                    "id":"chatcmpl-go", "object":"chat.completion.chunk",
                    "created":1, "model":"deepseek-v4-flash",
                    "choices":[{"index":0,"delta":{"content":"GO_STREAM_OK"},"finish_reason":null}]
                })
                .to_string(),
            ),
            SseEvent::data(
                serde_json::json!({
                    "id":"chatcmpl-go", "object":"chat.completion.chunk",
                    "created":1, "model":"deepseek-v4-flash",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":85,"completion_tokens":16,"total_tokens":101}
                })
                .to_string(),
            ),
            // OpenCode Go extension events: no `id`, so they are not valid
            // ChatCompletionChunks. The pre-[DONE] one previously killed the
            // stream with a serialization error; the post-[DONE] one is never
            // read but mirrors the real endpoint faithfully.
            SseEvent::data(
                serde_json::json!({
                    "choices":[], "x-opencode-type":"inference-cost",
                    "cost":"0.00001638",
                    "normalizedUsage":{"inputTokens":85,"outputTokens":16}
                })
                .to_string(),
            ),
            SseEvent::data("[DONE]"),
            SseEvent::data(serde_json::json!({"choices":[],"cost":"0"}).to_string()),
        ]),
    );
    let client = SamplingClient::new(SamplerConfig {
        api_key: Some("go-test-key".to_owned()),
        base_url: server.url(),
        model: "deepseek-v4-flash".to_owned(),
        auth_scheme: AuthScheme::Bearer,
        ..SamplerConfig::default()
    })
    .expect("OpenCode Go client builds");

    let response = sample_chat(
        &client,
        ConversationRequest::from_items(vec![ConversationItem::user("hello")]),
    )
    .await;
    assert_eq!(
        response
            .assistant()
            .expect("assistant response")
            .content
            .as_ref(),
        "GO_STREAM_OK"
    );

    let request = server
        .requests()
        .into_iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request captured");
    assert_eq!(request.authorization.as_deref(), Some("Bearer go-test-key"));
}
