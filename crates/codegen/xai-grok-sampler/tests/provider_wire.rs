// Added in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Provider-boundary wire acceptance tests using a real HTTP listener.

use indexmap::IndexMap;
use xai_grok_sampler::{ApiBackend, AuthScheme, SamplerConfig, SamplingClient};
use xai_grok_sampling_types::{
    ConversationItem, ConversationRequest, DoomLoopRecoveryPolicy, HostedTool, ToolSpec,
};
use xai_grok_test_support::{MockInferenceServer, MockModelEntry};

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
