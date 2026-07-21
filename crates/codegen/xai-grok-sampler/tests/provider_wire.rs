// Added in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Provider-boundary wire acceptance tests using a real HTTP listener.

use indexmap::IndexMap;
use std::time::Duration;

use xai_grok_sampler::{
    ApiBackend, AuthScheme, ChatMaxTokensField, ProviderCapabilities, ReasoningResponseField,
    RequestId, SamplerConfig, SamplingClient, WireQuirks, collect_response,
    stream_chat_completions,
};
use xai_grok_sampling_types::{
    ConversationItem, ConversationRequest, ConversationResponse, ConversationToolChoice,
    DoomLoopRecoveryPolicy, HostedTool, ToolSpec,
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
        HostedTool::WebSearch {
            allowed_domains: None,
        },
        HostedTool::XSearch,
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
            SseEvent::data(
                serde_json::json!({"choices":[],"cost":"0"}).to_string(),
            ),
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
