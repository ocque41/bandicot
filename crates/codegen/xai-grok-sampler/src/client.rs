// Modified in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! HTTP client for the xAI sampling APIs.
//!
//! Owns the `reqwest::Client`, default request headers, and per-method
//! defaults. Talks to three backend shapes:
//!
//! * Chat Completions (`/chat/completions`)
//! * Responses API (`/responses`)
//! * Anthropic Messages API (`/messages`)
//!
//! All trace-upload and URL-based header injection is intentionally
//! *not* here. The session is responsible for putting any per-request
//! headers (proxy auth, OTel context, etc.)
//! into [`SamplerConfig::extra_headers`] before constructing the client.

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use serde::Serialize;
use serde_json::Value;

use xai_grok_sampling_types::error::{parse_error_bytes, try_parse_stream_error};
use xai_grok_sampling_types::{
    CANONICAL_REASONING_EFFORT_METADATA_KEY, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, ConversationRequest, ConversationResponse, CreateResponseWrapper,
    DOOM_LOOP_CHECK_HEADER, InferenceTransport, MessagesRequestWrapper,
    OPENAI_PRIORITY_SERVICE_TIER, OPENAI_RESPONSES_MULTI_AGENT_BETA, ReasoningEffort,
    ResolvedServiceTier, ResponseModelMetadata, Result, SamplingError, build_messages_request,
    is_check_event, messages, rs,
};

use crate::config::{
    AuthScheme, ChatMaxTokensField, OriginClientInfo, ProviderCapabilities, ReasoningResponseField,
    SamplerConfig, WireQuirks,
};

// Re-export ApiBackend from the shared types crate for downstream callers.
pub use xai_grok_sampling_types::ApiBackend;

/// Process-level fallback for the `x-grok-client-identifier` header.
const DEFAULT_CLIENT_IDENTIFIER: &str = "grok-shell";

/// Product identifier baked into User-Agent strings.
const AGENT_PRODUCT: &str = "grok-shell";
const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 128_000;

/// Whether a sampling base URL is allowed to receive xAI-only wire extensions.
///
/// Keep this trust boundary sampler-local: this crate intentionally has no
/// shell dependency. The production CLI proxy is matched exactly (including
/// its `/v1` path boundary); all trusted hosts require HTTPS on the default
/// port, and `x.ai` uses a hostname-boundary check so suffix attacks such as
/// `api.x.ai.evil.example` are rejected.
fn is_first_party_xai_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" || parsed.port_or_known_default() != Some(443) {
        return false;
    }

    let host = parsed.host_str().unwrap_or_default();
    if host == "x.ai" || host.ends_with(".x.ai") {
        return true;
    }

    let path = parsed.path().trim_end_matches('/');
    host == "cli-chat-proxy.grok.com" && (path == "/v1" || path.starts_with("/v1/"))
}

fn hosted_multi_agent_limit(requested: Option<u32>, provider_max: Option<u32>) -> Option<u32> {
    match (requested, provider_max) {
        (Some(requested), Some(provider_max)) => Some(requested.min(provider_max)),
        (Some(requested), None) => Some(requested),
        (None, _) => None,
    }
}

fn request_uses_priority_service_tier(request_body: &Value) -> bool {
    request_body.get("service_tier").and_then(Value::as_str) == Some(OPENAI_PRIORITY_SERVICE_TIER)
}

fn remove_service_tier(request_body: &mut Value) {
    if let Some(body) = request_body.as_object_mut() {
        body.remove("service_tier");
    }
}

fn response_error_targets_service_tier(error: &Value) -> bool {
    if error.get("param").and_then(Value::as_str) == Some("service_tier") {
        return true;
    }

    error
        .get("code")
        .and_then(Value::as_str)
        .is_some_and(|code| code.to_ascii_lowercase().contains("service_tier"))
}

fn is_service_tier_rejection(status: reqwest::StatusCode, bytes: &[u8]) -> bool {
    if !matches!(
        status,
        reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY
    ) {
        return false;
    }

    let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };

    body.get("error")
        .is_some_and(response_error_targets_service_tier)
        || response_error_targets_service_tier(&body)
}

fn should_retry_without_priority_service_tier(
    status: reqwest::StatusCode,
    bytes: &[u8],
    request_body: &Value,
) -> bool {
    request_uses_priority_service_tier(request_body) && is_service_tier_rejection(status, bytes)
}

fn is_xai_only_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("x-grok-")
        || name.starts_with("x-xai-")
        || name == "x-compactions-remaining"
        || name == "x-compaction-at"
}

/// Per-request `x-grok-*` headers. Optional fields are skipped when empty/`None`.
struct GrokRequestHeaders<'a> {
    conv_id: &'a str,
    req_id: &'a str,
    model_id: &'a str,
    session_id: &'a str,
    turn_idx: Option<&'a str>,
    agent_id: &'a str,
    deployment_id: Option<&'a str>,
    user_id: Option<&'a str>,
}

impl GrokRequestHeaders<'_> {
    fn apply(
        &self,
        builder: reqwest::RequestBuilder,
        xai_wire_extensions: bool,
    ) -> reqwest::RequestBuilder {
        if !xai_wire_extensions {
            return builder;
        }
        let mut b = builder
            .header("x-grok-conv-id", self.conv_id)
            .header("x-grok-req-id", self.req_id)
            .header("x-grok-model-override", self.model_id)
            .header("x-grok-session-id", self.session_id)
            .header("x-grok-agent-id", self.agent_id);
        if let Some(idx) = self.turn_idx {
            b = b.header("x-grok-turn-idx", idx);
        }
        if let Some(id) = self.deployment_id.filter(|s| !s.is_empty()) {
            b = b.header("x-grok-deployment-id", id);
        }
        if let Some(id) = self.user_id.filter(|s| !s.is_empty()) {
            b = b.header("x-grok-user-id", id);
        }
        b
    }
}

/// Parse the `Retry-After` response header as delta-seconds.
/// Our inference backends only emit integer seconds (never HTTP-date),
/// so we only handle that form. HTTP-dates silently return `None` and
/// the caller falls back to exponential backoff.
/// Capped at 120s to prevent absurdly long sleeps from a misbehaving upstream.
/// Deserialize a Responses API SSE event, with a fallback for xAI-specific
/// tool types (e.g., `x_search`) that `async_openai` can't parse.
///
/// The API echoes the request's `tools` array in `ResponseCompleted` and
/// `ResponseCreated` events. If we sent `{"type": "x_search"}`, the response
/// includes it, and `rs::Tool` deserialization fails. On failure, we strip
/// unrecognized tools from the raw JSON and retry.
///
/// On `response.completed` / `response.incomplete`, this also rewrites
/// `response.usage.total_tokens` in place to the live context length
/// (`context_details.input_tokens + context_details.output_tokens`)
/// when the API emits the xAI-specific `context_details` field.
/// Async-openai's typed `ResponseUsage` doesn't model `context_details`,
/// so we peek the raw JSON for it. The cumulative `input_tokens` /
/// `output_tokens` / `cached_tokens` continue to flow from the typed
/// `ResponseUsage` unchanged so billing telemetry stays correct. When
/// the API doesn't emit `context_details` (older deployments) `total_tokens`
/// passes through unchanged.
#[derive(serde::Deserialize)]
struct ResponseEventEnvelope<'a> {
    #[serde(rename = "type", borrow)]
    event_type: &'a str,
}

fn response_event_type(data: &str) -> Option<&str> {
    serde_json::from_str::<ResponseEventEnvelope<'_>>(data)
        .ok()
        .map(|envelope| envelope.event_type)
}

/// True when an SSE `data` payload on the chat/completions wire is valid JSON
/// but cannot be a [`ChatCompletionChunk`] because the required `id` field is
/// absent. Providers use such payloads for out-of-band extension events that
/// carry no assistant content -- e.g. OpenCode Go emits an
/// `x-opencode-type: inference-cost` summary (`{"choices":[],"cost":...}`)
/// immediately before `[DONE]`. Like unknown Responses event types, these are
/// skipped so a provider extension cannot break an otherwise valid stream;
/// malformed JSON and malformed real chunks remain hard errors.
fn is_chat_chunk_extension_event(data: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    value.get("id").is_none()
}

fn is_known_response_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.created"
            | "response.in_progress"
            | "response.completed"
            | "response.failed"
            | "response.incomplete"
            | "response.output_item.added"
            | "response.output_item.done"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.delta"
            | "response.output_text.done"
            | "response.refusal.delta"
            | "response.refusal.done"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.file_search_call.in_progress"
            | "response.file_search_call.searching"
            | "response.file_search_call.completed"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.web_search_call.completed"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
            | "response.image_generation_call.completed"
            | "response.image_generation_call.generating"
            | "response.image_generation_call.in_progress"
            | "response.image_generation_call.partial_image"
            | "response.mcp_call_arguments.delta"
            | "response.mcp_call_arguments.done"
            | "response.mcp_call.completed"
            | "response.mcp_call.failed"
            | "response.mcp_call.in_progress"
            | "response.mcp_list_tools.completed"
            | "response.mcp_list_tools.failed"
            | "response.mcp_list_tools.in_progress"
            | "response.code_interpreter_call.in_progress"
            | "response.code_interpreter_call.interpreting"
            | "response.code_interpreter_call.completed"
            | "response.code_interpreter_call_code.delta"
            | "response.code_interpreter_call_code.done"
            | "response.output_text.annotation.added"
            | "response.queued"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done"
            | "error"
    )
}

/// Normalize OpenAI's inbound `max` token for async-openai 0.33 while
/// preserving the canonical value in process-local response metadata.
///
/// This touches only the exact `reasoning.effort == "max"` field. Malformed
/// response shapes and all other effort values remain unchanged so the typed
/// parser continues to reject malformed known responses strictly.
fn normalize_inbound_max_reasoning_effort(response: &mut serde_json::Value) -> bool {
    if response
        .pointer("/reasoning/effort")
        .and_then(serde_json::Value::as_str)
        != Some("max")
    {
        return false;
    }

    let Some(effort) = response.pointer_mut("/reasoning/effort") else {
        return false;
    };
    *effort = serde_json::Value::String("xhigh".to_owned());

    let Some(response) = response.as_object_mut() else {
        return true;
    };
    let metadata = response
        .entry("metadata")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if metadata.is_null() {
        *metadata = serde_json::Value::Object(Default::default());
    }
    if let Some(metadata) = metadata.as_object_mut() {
        metadata.insert(
            CANONICAL_REASONING_EFFORT_METADATA_KEY.to_owned(),
            serde_json::Value::String("max".to_owned()),
        );
    }

    true
}

/// Deserialize a non-streaming Responses body with the same narrow inbound
/// compatibility shim used by SSE events.
fn deserialize_response_body(bytes: &[u8]) -> serde_json::Result<rs::Response> {
    match serde_json::from_slice::<rs::Response>(bytes) {
        Ok(response) => Ok(response),
        Err(first_err) => {
            let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
                return Err(first_err);
            };
            if !normalize_inbound_max_reasoning_effort(&mut value) {
                return Err(first_err);
            }
            serde_json::from_value(value)
        }
    }
}

/// Deserialize a Responses event known to this client. Future event types are
/// ignored so adding a server-side event does not break an otherwise valid
/// stream. Malformed JSON and malformed known events remain hard errors.
/// Raw event data is never included in logs.
fn deserialize_response_event(
    data: &str,
    xai_wire_extensions: bool,
) -> Result<Option<rs::ResponseStreamEvent>> {
    let envelope = serde_json::from_str::<ResponseEventEnvelope<'_>>(data).map_err(|err| {
        tracing::error!(error = %err, "Malformed Responses API event envelope");
        SamplingError::Serialization(err)
    })?;
    if !is_known_response_event_type(envelope.event_type) {
        tracing::debug!(
            event_type = envelope.event_type,
            "Ignoring unknown Responses API stream event"
        );
        return Ok(None);
    }

    let mut event = match serde_json::from_str::<rs::ResponseStreamEvent>(data) {
        Ok(event) => event,
        Err(first_err) => {
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(data) {
                let normalized_max = value
                    .get_mut("response")
                    .is_some_and(normalize_inbound_max_reasoning_effort);
                if normalized_max
                    && let Ok(mut event) =
                        serde_json::from_value::<rs::ResponseStreamEvent>(value.clone())
                {
                    apply_terminal_event_overrides(&mut event, data, xai_wire_extensions);
                    return Ok(Some(event));
                }

                // xAI's verified first-party stream can echo tool definitions
                // that async-openai does not model. Keep this sanitizer behind
                // the existing provider trust boundary.
                // Strip tools that async_openai's rs::Tool can't deserialize
                // (e.g., xAI-specific "x_search"). Instead of maintaining a
                // hardcoded allowlist, try deserializing each tool entry —
                // if it fails, drop it.
                if xai_wire_extensions {
                    if let Some(tools) = value
                        .pointer_mut("/response/tools")
                        .and_then(|v| v.as_array_mut())
                    {
                        tools.retain(|t| serde_json::from_value::<rs::Tool>(t.clone()).is_ok());
                    }
                    if let Ok(mut event) = serde_json::from_value::<rs::ResponseStreamEvent>(value)
                    {
                        apply_terminal_event_overrides(&mut event, data, xai_wire_extensions);
                        return Ok(Some(event));
                    }
                }
            }
            tracing::error!(
                error = %first_err,
                event_type = envelope.event_type,
                "Failed to deserialize ResponseStreamEvent from stream"
            );
            return Err(SamplingError::Serialization(first_err));
        }
    };
    apply_terminal_event_overrides(&mut event, data, xai_wire_extensions);
    Ok(Some(event))
}

/// On terminal Responses API events (`response.completed` /
/// `response.incomplete`), rewrite `response.usage.total_tokens` to the
/// live context length when the wire includes
/// `response.usage.context_details.{input_tokens, output_tokens}`.
///
/// `total_tokens` drives the CLI's `/context` bar, the auto-compact
/// threshold, and `meta.totalTokens` on persisted sessions. Under
/// server-side multi-turn loops (e.g. `web_search`, `x_search`) the
/// wire's cumulative total inflates as the loop runs; `context_details`
/// reports the final turn's prompt + output tokens — the real live
/// context the model is sitting in. Billing fields
/// (`input_tokens`, `output_tokens`, `input_tokens_details.cached_tokens`,
/// `output_tokens_details.reasoning_tokens`) stay on the cumulative
/// wire values so telemetry is unaffected.
///
/// No-op when:
/// - the event is not terminal,
/// - `response.usage` is `None`,
/// - `context_details` is absent (older backends / non-loop responses),
/// - or either of `context_details.{input_tokens, output_tokens}` is
///   missing — we don't guess the missing half.
fn apply_terminal_event_overrides(
    event: &mut rs::ResponseStreamEvent,
    data: &str,
    xai_wire_extensions: bool,
) {
    if !xai_wire_extensions {
        return;
    }
    let response = match event {
        rs::ResponseStreamEvent::ResponseCompleted(e) => &mut e.response,
        rs::ResponseStreamEvent::ResponseIncomplete(e) => &mut e.response,
        _ => return,
    };
    // Re-parse for fields async_openai's types omit (context total, cost ticks).
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };
    // Stash cost ticks in metadata for stream_responses.
    if let Some(ticks) = xai_grok_sampling_types::reported_cost_ticks(
        value
            .pointer("/response/usage/cost_in_usd_ticks")
            .and_then(|v| v.as_i64()),
    ) {
        response
            .metadata
            .get_or_insert_with(Default::default)
            .insert(COST_USD_TICKS_METADATA_KEY.to_owned(), ticks.to_string());
    }
    let Some(usage) = response.usage.as_mut() else {
        return;
    };
    let Some(total) = extract_context_total(&value) else {
        return;
    };
    usage.total_tokens = total;
}

/// Metadata key for cost ticks past typed Response events.
pub(crate) const COST_USD_TICKS_METADATA_KEY: &str = "xai.cost_usd_ticks";

/// Read `response.usage.context_details.{input_tokens, output_tokens}`
/// from the parsed terminal-event JSON and return their sum. Returns `None`
/// if either field is missing or out of `u32` range.
fn extract_context_total(value: &serde_json::Value) -> Option<u32> {
    let cd = value.pointer("/response/usage/context_details")?;
    let i = u32::try_from(cd.get("input_tokens")?.as_u64()?).ok()?;
    let o = u32::try_from(cd.get("output_tokens")?.as_u64()?).ok()?;
    Some(i.saturating_add(o))
}

/// Record `success=false` + `error` on the active inference span when a stream
/// request fails before any response (transport/connect/TLS errors). Without
/// this the `#[instrument]` span closes with both fields Empty, so an outage
/// shows zero `success=false` and error-rate alerts never fire.
fn record_stream_request_failure(err: &reqwest::Error) {
    let span = tracing::Span::current();
    span.record("success", false);
    span.record("error", err.to_string().as_str());
}

fn extract_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|s| s.min(120))
}

fn parse_rate_limit_reset_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<f64>() {
        return (seconds.is_finite() && seconds >= 0.0).then(|| (seconds * 1000.0).ceil() as u64);
    }
    let mut total_ms = 0u64;
    let mut number_start = 0usize;
    let bytes = value.as_bytes();
    let mut parsed_any = false;
    while number_start < bytes.len() {
        let mut end = number_start;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        if end == number_start {
            return None;
        }
        let number = value[number_start..end].parse::<f64>().ok()?;
        let (factor, suffix_len) = if value[end..].starts_with("ms") {
            (1.0, 2)
        } else if value[end..].starts_with('s') {
            (1_000.0, 1)
        } else if value[end..].starts_with('m') {
            (60_000.0, 1)
        } else if value[end..].starts_with('h') {
            (3_600_000.0, 1)
        } else {
            return None;
        };
        total_ms = total_ms.saturating_add((number * factor).ceil() as u64);
        parsed_any = true;
        number_start = end + suffix_len;
    }
    parsed_any.then_some(total_ms)
}

fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn header_reset_ms(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    parse_rate_limit_reset_ms(headers.get(name)?.to_str().ok()?)
}

fn extract_rate_limit_metadata(
    headers: &reqwest::header::HeaderMap,
) -> Option<xai_grok_sampling_types::RateLimitMetadata> {
    use xai_grok_sampling_types::{RateLimitMetadata, RateLimitWindow};

    let metadata = RateLimitMetadata {
        requests: RateLimitWindow {
            limit: header_u64(headers, "x-ratelimit-limit-requests"),
            remaining: header_u64(headers, "x-ratelimit-remaining-requests"),
            reset_after_ms: header_reset_ms(headers, "x-ratelimit-reset-requests"),
        },
        tokens: RateLimitWindow {
            limit: header_u64(headers, "x-ratelimit-limit-tokens"),
            remaining: header_u64(headers, "x-ratelimit-remaining-tokens"),
            reset_after_ms: header_reset_ms(headers, "x-ratelimit-reset-tokens"),
        },
        project_tokens: RateLimitWindow {
            limit: header_u64(headers, "x-ratelimit-limit-project-tokens"),
            remaining: header_u64(headers, "x-ratelimit-remaining-project-tokens"),
            reset_after_ms: header_reset_ms(headers, "x-ratelimit-reset-project-tokens"),
        },
        retry_after_ms: extract_retry_after(headers).map(|seconds| seconds.saturating_mul(1000)),
    };
    (!metadata.is_empty()).then_some(metadata)
}

fn extract_should_retry(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    headers
        .get("x-should-retry")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            if s.eq_ignore_ascii_case("true") {
                Some(true)
            } else if s.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None // unknown value — treat as absent
            }
        })
}

fn extract_model_metadata(
    headers: &reqwest::header::HeaderMap,
    xai_wire_extensions: bool,
) -> Option<ResponseModelMetadata> {
    let context_window = xai_wire_extensions
        .then(|| {
            headers
                .get("x-grok-context-window")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
        })
        .flatten();

    let max_completion_tokens = xai_wire_extensions
        .then(|| {
            headers
                .get("x-grok-max-completion-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u32>().ok())
        })
        .flatten();

    let models_etag = headers
        .get("x-models-etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let rate_limits = extract_rate_limit_metadata(headers);

    if context_window.is_some()
        || max_completion_tokens.is_some()
        || models_etag.is_some()
        || rate_limits.is_some()
    {
        Some(ResponseModelMetadata {
            context_window,
            max_completion_tokens,
            models_etag,
            rate_limits,
        })
    } else {
        None
    }
}

/// Wrapper for streaming chat completion requests that adds `stream` and
/// `stream_options` fields without modifying the original `ChatCompletionRequest`.
///
/// Uses `#[serde(flatten)]` to inline all fields from the inner request,
/// allowing single-pass serialization instead of the previous two-pass
/// approach (serialize to `Value`, mutate, serialize to bytes).
#[derive(Serialize)]
struct StreamingChatRequest<'a> {
    #[serde(flatten)]
    inner: &'a ChatCompletionRequest,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// HTTP client for sampling. Cheap to clone; carries an `Arc`-backed
/// `reqwest::Client` and the default headers/request-defaults computed
/// from a [`SamplerConfig`] at construction time.
#[derive(Clone)]
pub struct SamplingClient {
    http: reqwest::Client,
    default_headers: HeaderMap,
    base_url: String,
    /// Derived once from `base_url`; false for OpenAI and every custom host.
    xai_wire_extensions: bool,
    defaults: ClientDefaults,
    /// Optional 401-attribution hook. The shell wires this to emit a
    /// structured event at every UNAUTHORIZED arm so 401s can be
    /// bucketed by stale-snapshot vs. live-token-rejected. `None` for
    /// sampler-only callers and tests.
    attribution_callback: Option<crate::attribution::SharedAttributionCallback>,
    /// Per-request bearer override. See `SamplerConfig::bearer_resolver`.
    bearer_resolver: Option<crate::config::SharedBearerResolver>,
    /// Per-request header injection (OTel traceparent).
    header_injector: Option<crate::config::SharedHeaderInjector>,
}

impl std::fmt::Debug for SamplingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamplingClient")
            .field("base_url", &self.base_url)
            .field("xai_wire_extensions", &self.xai_wire_extensions)
            .field("defaults", &self.defaults)
            .field(
                "has_attribution_callback",
                &self.attribution_callback.is_some(),
            )
            .field("has_bearer_resolver", &self.bearer_resolver.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
struct ClientDefaults {
    model: String,
    max_completion_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    api_backend: ApiBackend,
    transport: InferenceTransport,
    auth_scheme: AuthScheme,
    capabilities: ProviderCapabilities,
    effective_service_tier: ResolvedServiceTier,
    hosted_multi_agent: xai_grok_sampling_types::HostedMultiAgentConfig,
    wire_quirks: WireQuirks,
    reasoning_effort: Option<ReasoningEffort>,
    stream_tool_calls: bool,
    doom_loop_recovery: Option<xai_grok_sampling_types::DoomLoopRecoveryPolicy>,
}

// =============================================================================
// User-Agent helpers
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlatformInfo {
    os: String,
    arch: String,
}

impl PlatformInfo {
    fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            other => other,
        }
        .to_string();

        let arch = match std::env::consts::ARCH {
            "arm64" => "aarch64",
            "x86_64" => "x86_64",
            other => other,
        }
        .to_string();

        Self { os, arch }
    }
}

fn agent_version() -> String {
    xai_grok_version::VERSION.to_string()
}

/// Render a User-Agent string for the given origin client.
///
/// Mirrors the shell's `user_agent_string_for` but uses sampler-local
/// constants. The session typically owns the canonical User-Agent
/// rendering for process-wide HTTP clients; this helper is for
/// per-session sampling clients that want to override it.
pub fn user_agent_string_for(origin: &OriginClientInfo) -> String {
    let agent_version = agent_version();
    let platform = PlatformInfo::current();

    if origin.product == AGENT_PRODUCT && origin.version.as_deref() == Some(agent_version.as_str())
    {
        return format!(
            "{}/{} ({}; {})",
            AGENT_PRODUCT, agent_version, platform.os, platform.arch
        );
    }

    match origin.version.as_deref() {
        Some(origin_version) => format!(
            "{}/{} {}/{} ({}; {})",
            origin.product,
            origin_version,
            AGENT_PRODUCT,
            agent_version,
            platform.os,
            platform.arch
        ),
        None => format!(
            "{} {}/{} ({}; {})",
            origin.product, AGENT_PRODUCT, agent_version, platform.os, platform.arch
        ),
    }
}

// =============================================================================
// SamplingClient
// =============================================================================

impl SamplingClient {
    /// Construct a sampling client from a [`SamplerConfig`].
    ///
    /// Grabs the process-wide shared `reqwest::Client` (HTTP/2 by
    /// default, HTTP/1.1 when `config.force_http1` is set) and
    /// pre-computes the default request headers. This does not perform
    /// any network I/O.
    pub fn new(config: SamplerConfig) -> Result<Self> {
        let xai_wire_extensions = is_first_party_xai_url(&config.base_url);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(ref api_key) = config.api_key {
            match config.auth_scheme {
                AuthScheme::None => {}
                AuthScheme::XApiKey => {
                    let header_value = HeaderValue::from_str(api_key).map_err(|_| {
                        tracing::debug!(
                            "Invalid api_key: cannot be converted to a valid HTTP header"
                        );
                        SamplingError::Auth(
                            "Invalid api_key: cannot be converted to a valid HTTP header"
                                .to_string(),
                        )
                    })?;
                    headers.insert(HeaderName::from_static("x-api-key"), header_value);
                }
                AuthScheme::Bearer => {
                    let bearer = format!("Bearer {}", api_key);
                    let header_value = HeaderValue::from_str(&bearer).map_err(|_| {
                        tracing::debug!(
                            "Invalid api_key: cannot be converted to a valid HTTP Authorization header"
                        );
                        SamplingError::Auth(
                            "Invalid api_key: cannot be converted to a valid HTTP Authorization header"
                                .to_string(),
                        )
                    })?;
                    headers.insert(AUTHORIZATION, header_value);
                }
            }
        }

        // Apply all extra headers verbatim. This is the single
        // injection point for proxy-auth headers and any other URL- or
        // environment-specific headers the session decides to set.
        for (key, value) in &config.extra_headers {
            if !xai_wire_extensions && is_xai_only_header(key) {
                tracing::debug!(
                    header_name = key,
                    "Omitting xAI-only request header for a non-xAI endpoint"
                );
                continue;
            }
            let header_name = HeaderName::try_from(key.as_str())
                .map_err(|_| SamplingError::InvalidConfiguration("Invalid extra header name"))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|_| SamplingError::InvalidConfiguration("Invalid extra header value"))?;
            headers.insert(header_name, header_value);
        }

        if matches!(config.api_backend, ApiBackend::Responses)
            && config.capabilities.hosted_multi_agent.supported
            && config.hosted_multi_agent.enabled
        {
            headers.append(
                HeaderName::from_static("openai-beta"),
                HeaderValue::from_static(OPENAI_RESPONSES_MULTI_AGENT_BETA),
            );
        }

        if xai_wire_extensions {
            // xAI proxy/client identity headers never leave first-party hosts.
            if let Some(client_version) = config.client_version.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(client_version)
            {
                headers.insert(
                    HeaderName::from_static("x-grok-client-version"),
                    header_value,
                );
            }

            if let Some(deployment_id) = config.deployment_id.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(deployment_id)
            {
                headers.insert(
                    HeaderName::from_static("x-grok-deployment-id"),
                    header_value,
                );
            }

            if let Some(user_id) = config.user_id.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(user_id)
            {
                headers.insert(HeaderName::from_static("x-grok-user-id"), header_value);
            }

            let client_id = config
                .client_identifier
                .clone()
                .unwrap_or_else(|| DEFAULT_CLIENT_IDENTIFIER.to_string());
            if let Ok(header_value) = HeaderValue::from_str(&client_id) {
                headers.insert(
                    HeaderName::from_static("x-grok-client-identifier"),
                    header_value,
                );
            }
        }

        // Always set User-Agent: per-session origin if available, else fallback.
        {
            let ua_string = match config.origin_client.as_ref() {
                Some(origin) => user_agent_string_for(origin),
                None => user_agent_string_for(&OriginClientInfo {
                    product: AGENT_PRODUCT.to_string(),
                    version: Some(agent_version()),
                }),
            };
            if let Ok(v) = HeaderValue::from_str(&ua_string) {
                headers.insert(USER_AGENT, v);
            }
        }

        let http = if config.force_http1 {
            tracing::info!("Using HTTP/1.1 for sampling client (force_http1=true)");
            crate::shared_http::client_http1().map_err(SamplingError::Http)?
        } else {
            crate::shared_http::client().map_err(SamplingError::Http)?
        };

        tracing::info!(
            target: crate::sampling_log::TARGET,
            event = "client_new",
            base_url = %config.base_url,
            model = %config.model,
            api_backend = ?config.api_backend,
            auth_scheme = ?config.auth_scheme,
            // "unset" (not "none"): `ReasoningEffort::None` is a real wire value;
            // logging the absent Option as "none" looked like we were sending it.
            reasoning_effort = config.reasoning_effort.map_or("unset", |e| e.as_str()),
            has_api_key = config.api_key.is_some(),
            has_bearer_resolver = config.bearer_resolver.is_some(),
            has_authorization_header = headers.get(AUTHORIZATION).is_some(),
            has_x_api_key_header = headers.get(HeaderName::from_static("x-api-key")).is_some(),
        );

        let defaults = ClientDefaults {
            model: config.model,
            max_completion_tokens: config.max_completion_tokens,
            temperature: config.temperature,
            top_p: config.top_p,
            api_backend: config.api_backend,
            transport: config.transport,
            auth_scheme: config.auth_scheme,
            capabilities: config.capabilities,
            effective_service_tier: config.effective_service_tier,
            hosted_multi_agent: config.hosted_multi_agent,
            wire_quirks: config.wire_quirks,
            reasoning_effort: config.reasoning_effort,
            stream_tool_calls: config.stream_tool_calls,
            doom_loop_recovery: config.doom_loop_recovery,
        };

        Ok(Self {
            http,
            default_headers: headers,
            base_url: config.base_url,
            xai_wire_extensions,
            defaults,
            attribution_callback: config.attribution_callback,
            bearer_resolver: config.bearer_resolver,
            header_injector: config.header_injector,
        })
    }

    /// The configured API backend for this client.
    pub fn api_backend(&self) -> ApiBackend {
        self.defaults.api_backend.clone()
    }

    /// The I/O transport used for inference. Native transports do not call
    /// any of this client's HTTP request methods.
    pub fn transport(&self) -> InferenceTransport {
        self.defaults.transport
    }

    pub(crate) fn apple_defaults(&self) -> (String, Option<f32>, Option<u32>) {
        (
            self.defaults.model.clone(),
            self.defaults.temperature,
            self.defaults.max_completion_tokens,
        )
    }

    /// POST with default headers. Overrides auth from resolver if wired.
    fn post(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        let mut headers = self.default_headers.clone();
        if let Some(resolver) = &self.bearer_resolver
            && let Some(fresh) = resolver.current_bearer()
        {
            match self.defaults.auth_scheme {
                AuthScheme::None => {
                    headers.remove(AUTHORIZATION);
                    headers.remove(HeaderName::from_static("x-api-key"));
                }
                AuthScheme::XApiKey => {
                    headers.remove(AUTHORIZATION);
                    if let Ok(v) = HeaderValue::from_str(&fresh) {
                        headers.insert(HeaderName::from_static("x-api-key"), v);
                    }
                }
                AuthScheme::Bearer => {
                    headers.remove(HeaderName::from_static("x-api-key"));
                    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {fresh}")) {
                        headers.insert(AUTHORIZATION, v);
                    }
                }
            }
        }
        tracing::info!(
            target: crate::sampling_log::TARGET,
            event = "client_post",
            base_url = %self.base_url,
            model = %self.defaults.model,
            api_backend = ?self.defaults.api_backend,
            auth_scheme = ?self.defaults.auth_scheme,
            has_bearer_resolver = self.bearer_resolver.is_some(),
            has_authorization_header = headers.get(AUTHORIZATION).is_some(),
            has_x_api_key_header = headers.get(HeaderName::from_static("x-api-key")).is_some(),
        );
        if let Some(injector) = &self.header_injector {
            injector.inject(&mut headers);
        }
        if !self.xai_wire_extensions {
            let xai_only_names: Vec<HeaderName> = headers
                .keys()
                .filter(|name| is_xai_only_header(name.as_str()))
                .cloned()
                .collect();
            for name in xai_only_names {
                headers.remove(name);
            }
        }
        self.http.post(url).headers(headers)
    }

    /// Bearer prefix for 401 attribution. Prefers live resolver, falls back to default_headers.
    fn current_sent_bearer_prefix(&self) -> Option<String> {
        self.bearer_resolver
            .as_ref()
            .and_then(|r| r.current_bearer())
            .or_else(|| self.extract_sent_bearer())
            .map(|mut s| {
                s.truncate(crate::attribution::SENT_BEARER_PREFIX_LEN.min(s.len()));
                s
            })
    }

    /// Extract the bearer from `default_headers`, truncated to prefix length.
    /// Reads `x-api-key` (Anthropic Messages API) or `Authorization` (OpenAI-completions).
    fn extract_sent_bearer(&self) -> Option<String> {
        let raw = match self.defaults.auth_scheme {
            AuthScheme::None => None,
            AuthScheme::XApiKey => self
                .default_headers
                .get(HeaderName::from_static("x-api-key"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            AuthScheme::Bearer => self
                .default_headers
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(|s| s.to_string()),
        };
        raw.map(|mut s| {
            // Truncate in-place so we never materialize a heap-resident
            // copy of the full bearer outside the local stack of this
            // function. `String::truncate` operates on byte indices and
            // panics on a non-char-boundary cut; bearer tokens are
            // ASCII (per the `Authorization` and `x-api-key` header
            // grammars) so the byte index is always safe.
            s.truncate(crate::attribution::SENT_BEARER_PREFIX_LEN.min(s.len()));
            s
        })
    }

    /// Invoke the optional 401 attribution callback for one logical response
    /// from a verified xAI endpoint. Each UNAUTHORIZED arm calls this helper;
    /// non-xAI endpoints return without deriving a credential prefix. Emit
    /// happens at the lowest layer that saw the status, so higher layers must
    /// not emit a duplicate event.
    ///
    /// The bearer passed to the callback is already truncated to
    /// [`crate::attribution::SENT_BEARER_PREFIX_LEN`] characters by
    /// [`Self::extract_sent_bearer`]; the trait contract guarantees
    /// that callers downstream of this crate never see the full
    /// bearer.
    fn record_401_attribution(&self, consumer: crate::attribution::SamplingConsumer) {
        // Credential-prefix attribution is an internal xAI diagnostic. Never
        // derive or forward even a truncated OpenAI/custom-provider key.
        if !self.xai_wire_extensions {
            return;
        }
        if let Some(cb) = self.attribution_callback.as_ref() {
            let sent_prefix = self.current_sent_bearer_prefix();
            cb.record_401(consumer, sent_prefix.as_deref());
        }
    }

    pub fn auth_info(&self) -> crate::sampling_log::AuthInfo {
        let has_auth = self.bearer_resolver.as_ref().is_some_and(|r| {
            r.current_bearer()
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        }) || match self.defaults.auth_scheme {
            AuthScheme::None => false,
            AuthScheme::XApiKey => self
                .default_headers
                .contains_key(HeaderName::from_static("x-api-key")),
            AuthScheme::Bearer => self.default_headers.contains_key(AUTHORIZATION),
        };
        let auth_type = match (self.defaults.auth_scheme, has_auth) {
            (AuthScheme::None, _) => "none",
            (AuthScheme::XApiKey, true) => "x-api-key",
            (AuthScheme::Bearer, true) => "bearer",
            (_, false) => "none",
        };
        crate::sampling_log::AuthInfo {
            auth_type,
            auth_prefix: None,
        }
    }

    /// Check if a header name contains sensitive information that should be redacted.
    fn is_sensitive_header(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.contains("authorization")
            || lower.contains("api-key")
            || lower.contains("apikey")
            || lower.contains("token")
            || lower.contains("secret")
    }

    /// Format a single header for error messages, redacting sensitive values.
    fn format_header(name: &str, value: &str) -> String {
        let display_value = if Self::is_sensitive_header(name) {
            "[REDACTED]"
        } else {
            value
        };
        format!("  {}: {}", name, display_value)
    }

    /// Build request headers string for error messages (redacting sensitive values).
    fn format_request_headers(
        &self,
        x_grok_conv_id: &str,
        x_grok_req_id: &str,
        model_id: &str,
        include_accept: bool,
    ) -> Vec<String> {
        let mut req_headers: Vec<String> = self
            .default_headers
            .iter()
            .map(|(name, value)| {
                Self::format_header(name.as_str(), value.to_str().unwrap_or("[non-utf8]"))
            })
            .collect();

        if self.xai_wire_extensions {
            req_headers.push(Self::format_header("x-grok-conv-id", x_grok_conv_id));
            req_headers.push(Self::format_header("x-grok-req-id", x_grok_req_id));
            req_headers.push(Self::format_header("x-grok-model-override", model_id));
        }
        if include_accept {
            req_headers.push(Self::format_header("accept", "text/event-stream"));
        }
        req_headers
    }

    /// Build response headers string for error messages.
    fn format_response_headers(response: &reqwest::Response) -> Vec<String> {
        response
            .headers()
            .iter()
            .map(|(name, value)| Self::format_header(name.as_str(), &format!("{:?}", value)))
            .collect()
    }

    /// Log all headers from a request at debug level (redacting sensitive values).
    fn log_request_headers(request: &reqwest::Request, endpoint_name: &str) {
        for (name, value) in request.headers().iter() {
            let value_str = if Self::is_sensitive_header(name.as_str()) {
                "[REDACTED]"
            } else {
                value.to_str().unwrap_or("[non-utf8]")
            };
            tracing::debug!(
                header_name = %name,
                header_value = %value_str,
                "Request header ({})",
                endpoint_name
            );
        }
    }

    /// Build error context message based on error type and status code.
    /// Includes relevant request/response details depending on what the error is about.
    fn build_api_error_message(
        &self,
        status: reqwest::StatusCode,
        server_message: &str,
        endpoint: &str,
        req_headers: &[String],
        resp_headers: Option<&[String]>,
    ) -> String {
        let server_message_lower = server_message.to_lowercase();

        let mut context_parts = vec![server_message.to_string()];
        context_parts.push(format!("\nRequest URL: {}", endpoint));

        // Show headers if error mentions headers
        if server_message_lower.contains("header") {
            context_parts.push(format!("Request headers:\n{}", req_headers.join("\n")));
        }

        // Always show response headers for server errors
        if status.is_server_error()
            && let Some(resp_hdrs) = resp_headers
        {
            context_parts.push(format!("Response headers:\n{}", resp_hdrs.join("\n")));
        }

        context_parts.join("\n")
    }

    fn endpoint(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{base}/{path}")
    }

    fn apply_defaults(&self, mut request: ChatCompletionRequest) -> Result<ChatCompletionRequest> {
        if request.model.is_none() {
            request.model = Some(self.defaults.model.clone());
        }

        if request.max_tokens.is_none() {
            request.max_tokens = self.defaults.max_completion_tokens;
        }

        if request.temperature.is_none() {
            request.temperature = self.defaults.temperature;
        }

        if request.top_p.is_none() {
            request.top_p = self.defaults.top_p;
        }

        if request.reasoning_effort.is_none() {
            request.reasoning_effort = self.defaults.reasoning_effort;
        }

        Ok(request)
    }

    /// Remove fields implemented by xAI's Chat Completions extension before
    /// serializing a request for OpenAI or another custom provider. Standard
    /// OpenAI Chat accepts `reasoning_effort`, but not xAI's per-message
    /// `model_id` / `reasoning_content` fields or top-level
    /// `search_parameters` object.
    fn strip_non_xai_chat_fields(&self, request: &mut ChatCompletionRequest) {
        if self.xai_wire_extensions {
            return;
        }

        request.search_parameters = None;
        for message in &mut request.messages {
            message.model_id = None;
            if self.defaults.wire_quirks.reasoning_response_field
                == ReasoningResponseField::ReasoningContent
            {
                message.reasoning_content = None;
            }
        }
    }

    fn chat_payload(
        &self,
        request: &mut ChatCompletionRequest,
        streaming: bool,
    ) -> Result<serde_json::Value> {
        if !self.defaults.capabilities.tools {
            request.tools = None;
            request.tool_choice = None;
        } else if !self.defaults.wire_quirks.send_tool_choice {
            request.tool_choice = None;
        }
        if !self.defaults.capabilities.image_input
            && request.messages.iter().any(|message| {
                matches!(
                    &message.content,
                    xai_grok_sampling_types::MessageContent::Blocks(blocks)
                        if blocks.iter().any(|block| matches!(block, xai_grok_sampling_types::ChatContentBlock::ImageUrl { .. }))
                )
            })
        {
            return Err(SamplingError::InvalidConfiguration(
                "The selected model does not support image input",
            ));
        }

        let mut value = if streaming {
            serde_json::to_value(StreamingChatRequest {
                inner: request,
                stream: true,
                stream_options: self.defaults.wire_quirks.send_stream_options.then_some(
                    StreamOptions {
                        include_usage: true,
                    },
                ),
            })?
        } else {
            serde_json::to_value(request)?
        };
        let object = value
            .as_object_mut()
            .expect("ChatCompletionRequest serializes as an object");
        if self.defaults.wire_quirks.chat_max_tokens_field
            == ChatMaxTokensField::MaxCompletionTokens
            && let Some(max_tokens) = object.remove("max_tokens")
        {
            object.insert("max_completion_tokens".to_owned(), max_tokens);
        }
        if self.defaults.wire_quirks.reasoning_response_field == ReasoningResponseField::Reasoning
            && let Some(messages) = object
                .get_mut("messages")
                .and_then(serde_json::Value::as_array_mut)
        {
            for message in messages {
                if let Some(message) = message.as_object_mut()
                    && let Some(reasoning) = message.remove("reasoning_content")
                {
                    message.insert("reasoning".to_owned(), reasoning);
                }
            }
        }
        Ok(value)
    }

    async fn handle_response(&self, response: reqwest::Response) -> Result<ChatCompletionResponse> {
        let status = response.status();
        let model_metadata = extract_model_metadata(response.headers(), self.xai_wire_extensions);
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        let bytes = response.bytes().await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(crate::attribution::SamplingConsumer::ChatCompletions);
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401): {server_message}"
                )));
            }
            let message = parse_error_bytes(bytes.as_ref());
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        }

        let completion = serde_json::from_slice::<ChatCompletionResponse>(&bytes).map_err(|e| {
            tracing::error!(
                error = %e,
                payload_bytes = bytes.len(),
                "Failed to deserialize ChatCompletionResponse"
            );
            SamplingError::Serialization(e)
        })?;
        Ok(completion)
    }

    // =========================================================================
    // Chat Completions API
    // =========================================================================

    pub async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let mut payload = self.apply_defaults(request)?;
        self.strip_non_xai_chat_fields(&mut payload);
        let x_grok_conv_id = &payload.x_grok_conv_id.clone().unwrap_or_default();
        let x_grok_req_id = &payload.x_grok_req_id.clone().unwrap_or_default();
        let model_id = payload.model.clone().unwrap_or_default();
        let wire_payload = self.chat_payload(&mut payload, false)?;

        tracing::debug!(
            base_url = %self.base_url,
            model_id = %model_id,
            "Sending chat completion request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: payload.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: payload.x_grok_turn_idx.as_deref(),
            agent_id: payload.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: payload.x_grok_deployment_id.as_deref(),
            user_id: payload.x_grok_user_id.as_deref(),
        };
        let http_request = grok_headers
            .apply(
                self.post(self.endpoint("chat/completions")),
                self.xai_wire_extensions,
            )
            .json(&wire_payload);

        let response = http_request.send().await.map_err(|e| {
            // Log at debug level; errors are surfaced to the caller.
            tracing::debug!("HTTP request failed: {}", e);
            e
        })?;

        self.handle_response(response).await
    }

    /// Start a streaming chat completion request. Returns a stream of typed chunks.
    #[tracing::instrument(
        name = "http.chat_completion_stream",
        skip_all,
        fields(
            endpoint = %self.endpoint("chat/completions"),
            model_id = request.model.as_deref().unwrap_or(""),
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
            error = tracing::field::Empty,
        )
    )]
    pub async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(
        BoxStream<'static, Result<ChatCompletionChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        let mut payload = self.apply_defaults(request)?;
        self.strip_non_xai_chat_fields(&mut payload);
        let x_grok_conv_id = &payload.x_grok_conv_id.clone().unwrap_or_default();
        let x_grok_req_id = &payload.x_grok_req_id.clone().unwrap_or_default();
        let model_id = payload.model.clone().unwrap_or_default();
        let streaming_request = self.chat_payload(&mut payload, true)?;

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: payload.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: payload.x_grok_turn_idx.as_deref(),
            agent_id: payload.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: payload.x_grok_deployment_id.as_deref(),
            user_id: payload.x_grok_user_id.as_deref(),
        };
        let http_request = grok_headers
            .apply(
                self.post(self.endpoint("chat/completions")),
                self.xai_wire_extensions,
            )
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
            .json(&streaming_request);

        let built_request = http_request.build().map_err(|e| {
            tracing::error!("Failed to build HTTP request: {}", e);
            SamplingError::Http(e)
        })?;

        tracing::debug!(
            url = %built_request.url(),
            method = %built_request.method(),
            "Sending chat/completions request"
        );
        Self::log_request_headers(&built_request, "chat/completions");

        let response = self.http.execute(built_request).await.map_err(|e| {
            tracing::debug!("HTTP request failed: {}", e);
            record_stream_request_failure(&e);
            e
        })?;

        let status = response.status();
        let span = tracing::Span::current();
        span.record("status_code", status.as_u16() as i64);
        span.record("success", status.is_success());
        let model_metadata = extract_model_metadata(response.headers(), self.xai_wire_extensions);
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                span.record("error", "unauthorized (401)");
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::ChatCompletionsStream,
                );
                let endpoint = self.endpoint("chat/completions");
                let bytes = response.bytes().await?;
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401) from {endpoint}: {server_message}"
                )));
            }

            let req_headers =
                self.format_request_headers(x_grok_conv_id, x_grok_req_id, &model_id, true);
            let resp_headers = Self::format_response_headers(&response);
            let bytes = response.bytes().await?;
            let server_message = parse_error_bytes(bytes.as_ref());

            let message = self.build_api_error_message(
                status,
                &server_message,
                &self.endpoint("chat/completions"),
                &req_headers,
                Some(&resp_headers),
            );

            span.record("error", message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "chat/completions API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        }

        // Strip UTF-8 BOM if present: eventsource-stream 0.2.3 incorrectly slices BOM at byte 1 instead of 3.
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        // Turn raw bytes into SSE events
        let event_stream = byte_stream.eventsource();

        // Map SSE events into ChatCompletionChunk.
        // Uses `scan` so that `[DONE]` and transport errors both terminate the
        // stream (`None`). The first transport error is emitted to the consumer,
        // then subsequent polls return `None` -- preventing an infinite busy-loop
        // when the HTTP/2 connection drops and h2 keeps producing errors.
        // The scan item is an `Option`: `Some(None)` skips a provider extension
        // event (see `is_chat_chunk_extension_event`) without terminating the
        // stream (`filter_map` below), while an outer `None` still ends it.
        let chunks = event_stream
            .scan(false, |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "chat_completions",
                            payload_bytes = data.len(),
                        );

                        if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Some(Err(stream_error)))
                        } else if is_chat_chunk_extension_event(data) {
                            tracing::debug!(
                                payload_bytes = data.len(),
                                "Skipping provider extension event on chat/completions stream"
                            );
                            Some(None)
                        } else {
                            Some(Some(
                                serde_json::from_str::<ChatCompletionChunk>(data).map_err(|e| {
                                    tracing::error!(
                                        error = %e,
                                        payload_bytes = data.len(),
                                        "Failed to deserialize ChatCompletionChunk from stream"
                                    );
                                    SamplingError::Serialization(e)
                                }),
                            ))
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Some(Err(SamplingError::EventStreamError(e.to_string()))))
                    }
                };
                std::future::ready(item)
            })
            .filter_map(std::future::ready)
            .boxed();

        Ok((chunks, model_metadata))
    }

    // =========================================================================
    // Responses API
    // =========================================================================

    /// Apply default configuration to a Responses API request.
    fn apply_response_defaults(&self, request: &mut CreateResponseWrapper) -> Result<()> {
        // Apply model default if not specified
        if request.inner.model.is_none() {
            request.inner.model = Some(self.defaults.model.clone());
        }

        // Apply temperature default if not specified
        if request.inner.temperature.is_none() {
            request.inner.temperature = self.defaults.temperature;
        }

        // Apply top_p default if not specified
        if request.inner.top_p.is_none() {
            request.inner.top_p = self.defaults.top_p;
        }

        // Apply max_output_tokens default if not specified
        if request.inner.max_output_tokens.is_none() {
            request.inner.max_output_tokens = self.defaults.max_completion_tokens;
        }

        // Preserve an explicit request's canonical token. The typed
        // async-openai request cannot distinguish Max from Xhigh, so infer
        // from it only when the wrapper did not retain a canonical value.
        if request.reasoning_effort.is_none() {
            request.reasoning_effort = request
                .inner
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.clone())
                .map(ReasoningEffort::from_responses_api)
                .or(self.defaults.reasoning_effort);
        }
        if let Some(effort) = request.reasoning_effort {
            let reasoning = request.inner.reasoning.get_or_insert_with(Default::default);
            if reasoning.effort.is_none() {
                reasoning.effort = Some(effort.to_responses_api());
            }
        }

        // Set store to false if not specified (default is true, but that breaks ZDR compliance)
        if request.inner.store.is_none() {
            request.inner.store = Some(false);
        }

        // Include encrypted reasoning content if not specified
        let includes = request.inner.include.get_or_insert_with(Vec::new);
        if !includes.contains(&rs::IncludeEnum::ReasoningEncryptedContent) {
            includes.push(rs::IncludeEnum::ReasoningEncryptedContent);
        }

        Ok(())
    }

    /// Restore the exact per-request canonical effort at the final JSON
    /// boundary. async-openai 0.33 stages `Max` as `Xhigh`, but a client
    /// default must never rewrite an explicit request's `Xhigh` to `max`.
    fn patch_response_reasoning_effort(
        request_body: &mut serde_json::Value,
        reasoning_effort: Option<ReasoningEffort>,
    ) {
        if let Some(effort) = reasoning_effort
            && let Some(reasoning) = request_body
                .get_mut("reasoning")
                .and_then(serde_json::Value::as_object_mut)
        {
            reasoning.insert("effort".to_owned(), serde_json::json!(effort.as_str()));
        }
    }

    fn finalize_response_body(
        &self,
        request_body: &mut serde_json::Value,
        reasoning_effort: Option<ReasoningEffort>,
        streaming: bool,
        extra_raw_tools: Vec<serde_json::Value>,
    ) {
        self.apply_provider_request_extensions(request_body, streaming, extra_raw_tools);
        xai_grok_sampling_types::patch_reasoning_text_types(request_body);
        Self::patch_response_reasoning_effort(request_body, reasoning_effort);
    }

    fn apply_provider_request_extensions(
        &self,
        request_body: &mut serde_json::Value,
        streaming: bool,
        extra_raw_tools: Vec<serde_json::Value>,
    ) {
        if streaming && self.xai_wire_extensions && self.defaults.stream_tool_calls {
            request_body["stream_tool_calls"] = serde_json::json!(true);
        }
        if self.xai_wire_extensions && !extra_raw_tools.is_empty() {
            if let Some(tools) = request_body
                .get_mut("tools")
                .and_then(serde_json::Value::as_array_mut)
            {
                tools.extend(extra_raw_tools);
            } else {
                request_body["tools"] = serde_json::Value::Array(extra_raw_tools);
            }
        }
        if matches!(self.defaults.api_backend, ApiBackend::Responses)
            && self.defaults.capabilities.service_tiers.priority
            && self.defaults.effective_service_tier.responses_wire_value()
                == Some(OPENAI_PRIORITY_SERVICE_TIER)
        {
            request_body["service_tier"] = serde_json::json!(OPENAI_PRIORITY_SERVICE_TIER);
        }
        if matches!(self.defaults.api_backend, ApiBackend::Responses)
            && self.defaults.capabilities.hosted_multi_agent.supported
            && self.defaults.hosted_multi_agent.enabled
        {
            let mut multi_agent = serde_json::json!({ "enabled": true });
            if let Some(max) = hosted_multi_agent_limit(
                self.defaults.hosted_multi_agent.max_concurrent_subagents,
                self.defaults
                    .capabilities
                    .hosted_multi_agent
                    .max_concurrent_subagents,
            ) {
                multi_agent["max_concurrent_subagents"] = serde_json::json!(max);
            }
            request_body["multi_agent"] = multi_agent;
        }
        if matches!(self.defaults.api_backend, ApiBackend::Responses)
            && self.defaults.capabilities.prompt_cache
        {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            "bandicot-prompt-cache-v1".hash(&mut hasher);
            self.base_url.hash(&mut hasher);
            self.defaults.model.hash(&mut hasher);
            request_body["prompt_cache_key"] =
                serde_json::json!(format!("bandicot-v1-{:016x}", hasher.finish()));
        }
    }

    /// Create a response using the Responses API (non-streaming).
    ///
    /// This uses the Responses API format which provides a simpler interface
    /// for multi-turn conversations and tool calling.
    pub async fn create_response(
        &self,
        mut request: CreateResponseWrapper,
    ) -> Result<rs::Response> {
        self.apply_response_defaults(&mut request)?;

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone().unwrap_or_default();

        // The trace field is process-local: it is consumed by upstream
        // session code (which may upload a payload artifact) and is not
        // forwarded by the sampler. Drop it before we send.
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = %model_id,
            "Sending Responses API request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let mut request_body = serde_json::to_value(&request.inner).map_err(|e| {
            tracing::error!("Failed to serialize responses request: {}", e);
            SamplingError::Serialization(e)
        })?;
        // async-openai's ReasoningTextContent struct omits the `type`
        // discriminator that the Responses API requires on input. Patch
        // it in post-serialize. This is the last surviving piece of the
        // old raw_output machinery.
        self.finalize_response_body(
            &mut request_body,
            request.reasoning_effort,
            false,
            Vec::new(),
        );
        let mut retried_without_priority_service_tier = false;
        let (status, model_metadata, retry_after_secs, should_retry, bytes) = loop {
            let http_request = grok_headers
                .apply(
                    self.post(self.endpoint("responses")),
                    self.xai_wire_extensions,
                )
                .json(&request_body);

            let response = http_request.send().await.map_err(|e| {
                tracing::debug!("HTTP request failed: {}", e);
                e
            })?;

            let status = response.status();
            let model_metadata =
                extract_model_metadata(response.headers(), self.xai_wire_extensions);
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let bytes = response.bytes().await?;

            if !retried_without_priority_service_tier
                && should_retry_without_priority_service_tier(status, bytes.as_ref(), &request_body)
            {
                remove_service_tier(&mut request_body);
                retried_without_priority_service_tier = true;
                tracing::warn!(
                    status = %status,
                    model_id = %model_id,
                    "Responses API rejected priority service_tier; retrying once without service_tier"
                );
                continue;
            }

            break (
                status,
                model_metadata,
                retry_after_secs,
                should_retry,
                bytes,
            );
        };

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(crate::attribution::SamplingConsumer::Responses);
                let endpoint = self.endpoint("responses");
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401) from {endpoint}: {server_message}"
                )));
            }

            let req_headers =
                self.format_request_headers(x_grok_conv_id, x_grok_req_id, &model_id, false);
            let server_message = parse_error_bytes(bytes.as_ref());

            let message = self.build_api_error_message(
                status,
                &server_message,
                &self.endpoint("responses"),
                &req_headers,
                None,
            );
            tracing::warn!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "responses API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        }

        let response_obj = deserialize_response_body(&bytes).map_err(|e| {
            tracing::error!(
                error = %e,
                payload_bytes = bytes.len(),
                "Failed to deserialize rs::Response"
            );
            SamplingError::Serialization(e)
        })?;
        Ok(response_obj)
    }

    /// Create a streaming response using the Responses API.
    ///
    /// Returns a stream of `rs::ResponseStreamEvent` which includes events like:
    /// - `response.created` - Initial response object
    /// - `response.output_text.delta` - Text content deltas
    /// - `response.function_call_arguments.delta` - Function call argument deltas
    /// - `response.completed` - Final response with all output
    ///
    /// The third tuple element is a per-request doom-loop signal collector,
    /// `Some` only when `SamplerConfig::doom_loop_recovery` is set — the same
    /// gate that adds the opt-in `x-grok-doom-loop-check` request header, so
    /// header and parse protection cannot drift apart. It is filled by the
    /// SSE decoder as the server reports triggers and is meant to be handed
    /// to `stream_responses` so the signals land on the final
    /// `ConversationResponse`.
    #[tracing::instrument(
        name = "http.create_response_stream",
        skip_all,
        fields(
            endpoint = %self.endpoint("responses"),
            model_id = request.inner.model.as_deref().unwrap_or(""),
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
            error = tracing::field::Empty,
        )
    )]
    #[allow(clippy::type_complexity)]
    pub async fn create_response_stream(
        &self,
        mut request: CreateResponseWrapper,
    ) -> Result<(
        BoxStream<'static, Result<rs::ResponseStreamEvent>>,
        Option<ResponseModelMetadata>,
        Option<crate::doom_loop::DoomLoopSignalCollector>,
    )> {
        self.apply_response_defaults(&mut request)?;

        // Enable streaming
        request.inner.stream = Some(true);

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone().unwrap_or_default();

        // Drop process-local trace data (see note in `create_response`).
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = model_id.as_str(),
            "Sending responses API stream request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let extra_tool_entries = std::mem::take(&mut request.extra_tool_entries);
        let mut request_body = serde_json::to_value(&request.inner).map_err(|e| {
            tracing::error!("Failed to serialize responses request: {}", e);
            SamplingError::Serialization(e)
        })?;
        // xAI-only body extensions are applied only after the provider trust
        // boundary has been evaluated from the configured base URL.
        self.finalize_response_body(
            &mut request_body,
            request.reasoning_effort,
            true,
            extra_tool_entries,
        );
        let span = tracing::Span::current();
        let mut retried_without_priority_service_tier = false;
        let (response, doom_loop) = loop {
            // Fresh per attempt so signals never leak across retries; `None`
            // (check disabled) sends no header and does no peek work per event.
            let doom_loop = self
                .xai_wire_extensions
                .then(|| {
                    self.defaults
                        .doom_loop_recovery
                        .map(crate::doom_loop::DoomLoopSignalCollector::new)
                })
                .flatten();
            let mut http_request = grok_headers
                .apply(
                    self.post(self.endpoint("responses")),
                    self.xai_wire_extensions,
                )
                .header(ACCEPT, HeaderValue::from_static("text/event-stream"));
            if doom_loop.is_some() {
                // Presence opts in; the server ignores the value.
                http_request = http_request.header(DOOM_LOOP_CHECK_HEADER, "true");
            }
            let http_request = http_request.json(&request_body);

            let built_request = http_request.build().map_err(|e| {
                tracing::error!("Failed to build HTTP request: {}", e);
                SamplingError::Http(e)
            })?;

            tracing::debug!(
                url = %built_request.url(),
                method = %built_request.method(),
                "Sending responses API stream request"
            );
            Self::log_request_headers(&built_request, "responses");

            let response = self.http.execute(built_request).await.map_err(|e| {
                tracing::debug!("HTTP request failed: {}", e);
                record_stream_request_failure(&e);
                e
            })?;

            let status = response.status();
            span.record("status_code", status.as_u16() as i64);
            span.record("success", status.is_success());
            if status.is_success() {
                break (response, doom_loop);
            }

            if status == reqwest::StatusCode::UNAUTHORIZED {
                span.record("error", "unauthorized (401)");
                self.record_401_attribution(crate::attribution::SamplingConsumer::ResponsesStream);
                let endpoint = self.endpoint("responses");
                let bytes = response.bytes().await?;
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401) from {endpoint}: {server_message}"
                )));
            }

            let model_metadata =
                extract_model_metadata(response.headers(), self.xai_wire_extensions);
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let req_headers =
                self.format_request_headers(x_grok_conv_id, x_grok_req_id, &model_id, true);
            let resp_headers = Self::format_response_headers(&response);
            let bytes = response.bytes().await?;
            let server_message = parse_error_bytes(bytes.as_ref());

            if !retried_without_priority_service_tier
                && should_retry_without_priority_service_tier(status, bytes.as_ref(), &request_body)
            {
                remove_service_tier(&mut request_body);
                retried_without_priority_service_tier = true;
                tracing::warn!(
                    status = %status,
                    model_id = %model_id,
                    "Responses API stream rejected priority service_tier; retrying once without service_tier"
                );
                continue;
            }

            let message = self.build_api_error_message(
                status,
                &server_message,
                &self.endpoint("responses"),
                &req_headers,
                Some(&resp_headers),
            );

            span.record("error", message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "responses API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        };

        let model_metadata = extract_model_metadata(response.headers(), self.xai_wire_extensions);

        // Strip UTF-8 BOM if present
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        // Turn raw bytes into SSE events
        let event_stream = byte_stream.eventsource();

        let doom_loop_for_stream = doom_loop.clone();
        let xai_wire_extensions = self.xai_wire_extensions;

        // The scan item is an `Option`: `Some(None)` skips an absorbed
        // doom-loop event without terminating the stream (`filter_map`
        // below), while an outer `None` still ends it.
        let events = event_stream
            .scan(false, move |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "responses",
                            event_type = response_event_type(data).unwrap_or("malformed"),
                            payload_bytes = data.len(),
                        );

                        // Intercept the non-standard doom-loop event before
                        // typed deserialization; async-openai's event enum
                        // does not know it and would fail to parse it. With
                        // the check disabled, the shared name-or-payload-type
                        // predicate guards against a server emitting it
                        // despite no opt-in (rollout skew), named or not.
                        let swallow = match (xai_wire_extensions, &doom_loop_for_stream) {
                            (true, Some(collector)) => collector.absorb(&event.event, data),
                            (true, None) => is_check_event(&event.event, data),
                            (false, _) => false,
                        };
                        if swallow {
                            Some(None)
                        } else if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Some(Err(stream_error)))
                        } else {
                            match deserialize_response_event(data, xai_wire_extensions) {
                                Ok(Some(event)) => Some(Some(Ok(event))),
                                Ok(None) => Some(None),
                                Err(err) => Some(Some(Err(err))),
                            }
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Some(Err(SamplingError::EventStreamError(e.to_string()))))
                    }
                };
                std::future::ready(item)
            })
            .filter_map(std::future::ready)
            .boxed();

        Ok((events, model_metadata, doom_loop))
    }

    // =========================================================================
    // Anthropic Messages API
    // =========================================================================

    /// Apply default configuration to a Messages API request.
    fn apply_message_defaults(&self, request: &mut MessagesRequestWrapper) -> Result<()> {
        // Apply model default if not specified
        if request.inner.model.is_empty() {
            request.inner.model = self.defaults.model.clone();
        }

        if request.inner.max_tokens == 0 {
            request.inner.max_tokens = self
                .defaults
                .max_completion_tokens
                .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);
        }

        // Apply temperature default if not specified
        if request.inner.temperature.is_none() {
            request.inner.temperature = self.defaults.temperature;
        }

        // Apply top_p default if not specified
        if request.inner.top_p.is_none() {
            request.inner.top_p = self.defaults.top_p;
        }

        Ok(())
    }

    /// Create a message using the Anthropic Messages API (non-streaming).
    pub async fn create_message(
        &self,
        mut request: MessagesRequestWrapper,
    ) -> Result<messages::MessagesResponse> {
        self.apply_message_defaults(&mut request)?;

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone();

        // Drop process-local trace data.
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = %model_id,
            "Sending Messages API request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let http_request = grok_headers
            .apply(
                self.post(self.endpoint("messages")),
                self.xai_wire_extensions,
            )
            .json(&request.inner);

        let response = http_request.send().await.map_err(|e| {
            tracing::debug!("HTTP request failed: {}", e);
            e
        })?;

        let status = response.status();
        let model_metadata = extract_model_metadata(response.headers(), self.xai_wire_extensions);
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        let bytes = response.bytes().await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(crate::attribution::SamplingConsumer::Messages);
                let endpoint = self.endpoint("messages");
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401) from {endpoint}: {server_message}"
                )));
            }

            let req_headers =
                self.format_request_headers(x_grok_conv_id, x_grok_req_id, &model_id, false);
            let server_message = parse_error_bytes(bytes.as_ref());

            let message = self.build_api_error_message(
                status,
                &server_message,
                &self.endpoint("messages"),
                &req_headers,
                None,
            );
            tracing::warn!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "messages API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        }

        let response_obj =
            serde_json::from_slice::<messages::MessagesResponse>(&bytes).map_err(|e| {
                tracing::error!(
                    error = %e,
                    payload_bytes = bytes.len(),
                    "Failed to deserialize MessagesResponse"
                );
                SamplingError::Serialization(e)
            })?;
        Ok(response_obj)
    }

    /// Create a streaming message using the Anthropic Messages API.
    ///
    /// Returns a stream of `MessageStreamEvent` which includes events like:
    /// - `message_start` - Initial message object
    /// - `content_block_start` / `content_block_delta` / `content_block_stop` - Content blocks
    /// - `message_delta` / `message_stop` - Final message with stop reason
    #[tracing::instrument(
        name = "http.create_message_stream",
        skip_all,
        fields(
            endpoint = %self.endpoint("messages"),
            model_id = request.inner.model.as_str(),
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
            error = tracing::field::Empty,
        )
    )]
    pub async fn create_message_stream(
        &self,
        mut request: MessagesRequestWrapper,
    ) -> Result<(
        BoxStream<'static, Result<messages::MessageStreamEvent>>,
        Option<ResponseModelMetadata>,
    )> {
        self.apply_message_defaults(&mut request)?;

        // Enable streaming
        request.inner.stream = Some(true);

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone();

        // Drop process-local trace data.
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = model_id.as_str(),
            "Sending Messages API stream request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let http_request = grok_headers
            .apply(
                self.post(self.endpoint("messages")),
                self.xai_wire_extensions,
            )
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"))
            .json(&request.inner);

        let built_request = http_request.build().map_err(|e| {
            tracing::error!("Failed to build HTTP request: {}", e);
            SamplingError::Http(e)
        })?;

        tracing::debug!(
            url = %built_request.url(),
            method = %built_request.method(),
            "Sending messages API stream request"
        );
        Self::log_request_headers(&built_request, "messages");

        let response = self.http.execute(built_request).await.map_err(|e| {
            tracing::debug!("HTTP request failed: {}", e);
            record_stream_request_failure(&e);
            e
        })?;

        let status = response.status();
        let span = tracing::Span::current();
        span.record("status_code", status.as_u16() as i64);
        span.record("success", status.is_success());
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                span.record("error", "unauthorized (401)");
                self.record_401_attribution(crate::attribution::SamplingConsumer::MessagesStream);
                let endpoint = self.endpoint("messages");
                let bytes = response.bytes().await?;
                let server_message = parse_error_bytes(bytes.as_ref());
                return Err(SamplingError::Auth(format!(
                    "Unauthorized (401) from {endpoint}: {server_message}"
                )));
            }
            let model_metadata =
                extract_model_metadata(response.headers(), self.xai_wire_extensions);
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let req_headers =
                self.format_request_headers(x_grok_conv_id, x_grok_req_id, &model_id, true);
            let resp_headers = Self::format_response_headers(&response);
            let bytes = response.bytes().await?;
            let server_message = parse_error_bytes(bytes.as_ref());

            let message = self.build_api_error_message(
                status,
                &server_message,
                &self.endpoint("messages"),
                &req_headers,
                Some(&resp_headers),
            );

            span.record("error", message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "messages API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
            });
        }

        let model_metadata = extract_model_metadata(response.headers(), self.xai_wire_extensions);

        // Strip UTF-8 BOM if present
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        // Turn raw bytes into SSE events
        let event_stream = byte_stream.eventsource();

        // Map SSE events into MessageStreamEvent.
        // Uses `scan` so transport errors terminate the stream after the first
        // error (same pattern as `chat_completion_stream`).
        let events = event_stream
            .scan(false, |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "messages",
                            payload_bytes = data.len(),
                        );

                        if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Err(stream_error))
                        } else {
                            Some(
                                serde_json::from_str::<messages::MessageStreamEvent>(data).map_err(
                                    |e| {
                                        tracing::error!(
                                            error = %e,
                                            payload_bytes = data.len(),
                                            "Failed to deserialize MessageStreamEvent from stream"
                                        );
                                        SamplingError::Serialization(e)
                                    },
                                ),
                            )
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Err(SamplingError::EventStreamError(e.to_string())))
                    }
                };
                std::future::ready(item)
            })
            .boxed();

        Ok((events, model_metadata))
    }

    // =========================================================================
    // Unified Conversation API
    // =========================================================================

    /// Apply default configuration to a ConversationRequest.
    fn apply_conversation_defaults(&self, request: &mut ConversationRequest) -> Result<()> {
        if !self.defaults.capabilities.image_input
            && request.items.iter().any(|item| match item {
                xai_grok_sampling_types::ConversationItem::User(user) => user
                    .content
                    .iter()
                    .any(|part| matches!(part, xai_grok_sampling_types::ContentPart::Image { .. })),
                xai_grok_sampling_types::ConversationItem::ToolResult(result) => {
                    !result.images.is_empty()
                }
                _ => false,
            })
        {
            return Err(SamplingError::InvalidConfiguration(
                "The selected model does not support image input",
            ));
        }
        if request.model.is_none() {
            request.model = Some(self.defaults.model.clone());
        }

        if request.temperature.is_none() {
            request.temperature = self.defaults.temperature;
        }

        if request.top_p.is_none() {
            request.top_p = self.defaults.top_p;
        }

        if request.max_output_tokens.is_none() {
            request.max_output_tokens = self.defaults.max_completion_tokens;
        }

        if request.reasoning_effort.is_none() {
            request.reasoning_effort = self.defaults.reasoning_effort;
        }

        if !self.defaults.capabilities.tools {
            request.tools.clear();
            request.hosted_tools.clear();
        }

        Ok(())
    }

    /// Remove xAI-only hosted tools before building a non-xAI Responses
    /// request. This must happen before typed conversion: tool-name collision
    /// resolution runs during conversion, and leaving `HostedTool::XSearch`
    /// present there would incorrectly suppress a legitimate OpenAI/custom
    /// function named `x_search` even though the raw hosted tool is omitted
    /// later at the wire boundary.
    fn strip_non_xai_hosted_tools(&self, request: &mut ConversationRequest) {
        if !self.xai_wire_extensions {
            request.hosted_tools.retain(|tool| {
                !matches!(tool, xai_grok_sampling_types::HostedTool::XSearch { .. })
            });
        }
    }

    /// Send a conversation request using the Chat Completions API (streaming).
    ///
    /// Converts the `ConversationRequest` to `ChatCompletionRequest` internally.
    /// Returns the stream and any model metadata extracted from response headers.
    pub async fn conversation_stream(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<ChatCompletionChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let mut chat_request: ChatCompletionRequest = request.into();
        if let Some(trace) = trace {
            chat_request.trace = Some(trace);
        }

        self.chat_completion_stream(chat_request).await
    }

    /// Send a conversation request using the Chat Completions API (non-streaming).
    ///
    /// Converts the `ConversationRequest` to `ChatCompletionRequest` internally.
    pub async fn conversation(
        &self,
        mut request: ConversationRequest,
    ) -> Result<ChatCompletionResponse> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let mut chat_request: ChatCompletionRequest = request.into();
        if let Some(trace) = trace {
            chat_request.trace = Some(trace);
        }

        self.chat_completion(chat_request).await
    }

    /// Send a conversation request using the Responses API (streaming).
    ///
    /// Converts the `ConversationRequest` to Responses API format internally.
    /// The third tuple element is the per-request doom-loop signal collector
    /// (see [`Self::create_response_stream`]); callers that don't consume the
    /// signals can ignore it.
    #[allow(clippy::type_complexity)]
    pub async fn conversation_stream_responses(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<rs::ResponseStreamEvent>>,
        Option<ResponseModelMetadata>,
        Option<crate::doom_loop::DoomLoopSignalCollector>,
    )> {
        self.apply_conversation_defaults(&mut request)?;
        self.strip_non_xai_hosted_tools(&mut request);

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        // Collect xAI-specific tools that can't be expressed via rs::Tool
        // (e.g., x_search). These are injected as raw JSON after serialization.
        let extra_tools = if self.xai_wire_extensions {
            xai_grok_sampling_types::extra_tool_entries(&request.hosted_tools)
        } else {
            Vec::new()
        };

        let mut wrapper: CreateResponseWrapper = (&request).into();
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_agent_id = x_grok_agent_id;
        wrapper.extra_tool_entries = extra_tools;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_response_stream(wrapper).await
    }

    /// Send a conversation request using the Responses API (non-streaming).
    ///
    /// Converts the `ConversationRequest` to Responses API format internally.
    pub async fn conversation_responses(
        &self,
        mut request: ConversationRequest,
    ) -> Result<rs::Response> {
        self.apply_conversation_defaults(&mut request)?;
        self.strip_non_xai_hosted_tools(&mut request);

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        let mut wrapper: CreateResponseWrapper = (&request).into();
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_agent_id = x_grok_agent_id;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_response(wrapper).await
    }

    /// Send a conversation request using the Anthropic Messages API (streaming).
    ///
    /// Converts the `ConversationRequest` to Messages API format internally.
    pub async fn conversation_stream_messages(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<messages::MessageStreamEvent>>,
        Option<ResponseModelMetadata>,
    )> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        let messages_request = build_messages_request(&request);

        let mut wrapper = MessagesRequestWrapper::new(messages_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_agent_id = x_grok_agent_id;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_message_stream(wrapper).await
    }

    /// Send a conversation request using the Anthropic Messages API (non-streaming).
    ///
    /// Converts the `ConversationRequest` to Messages API format internally.
    pub async fn conversation_messages(
        &self,
        mut request: ConversationRequest,
    ) -> Result<messages::MessagesResponse> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        let messages_request = build_messages_request(&request);

        let mut wrapper = MessagesRequestWrapper::new(messages_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_agent_id = x_grok_agent_id;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_message(wrapper).await
    }

    /// Backend-aware streaming call that collects the full response.
    pub async fn conversation_collect(
        &self,
        request: ConversationRequest,
    ) -> Result<ConversationResponse> {
        let request_id = crate::types::RequestId::random();
        let idle_timeout = std::time::Duration::from_secs(300);
        let result = match self.api_backend() {
            ApiBackend::ChatCompletions => {
                let (raw, meta) = self.conversation_stream(request).await?;
                let events =
                    crate::stream::stream_chat_completions(raw, meta, request_id, idle_timeout);
                crate::stream::collect_response(events).await
            }
            ApiBackend::Responses => {
                let (raw, meta, doom_loop) = self.conversation_stream_responses(request).await?;
                let events =
                    crate::stream::stream_responses(raw, meta, request_id, idle_timeout, doom_loop);
                crate::stream::collect_response(events).await
            }
            ApiBackend::Messages => {
                let (raw, meta) = self.conversation_stream_messages(request).await?;
                let events = crate::stream::stream_messages(raw, meta, request_id, idle_timeout);
                crate::stream::collect_response(events).await
            }
        };
        result
            .map(|(response, _metrics)| response)
            .map_err(|info| SamplingError::Api {
                status: info
                    .status_code
                    .and_then(|c| reqwest::StatusCode::from_u16(c).ok())
                    .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
                message: info.message,
                model_metadata: info.model_metadata,
                retry_after_secs: info.retry_after_secs,
                should_retry: None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use xai_grok_sampling_types::ConversationItem;
    use xai_grok_sampling_types::types::ChatRequestMessage;

    fn known_xai_response_event(data: &str) -> rs::ResponseStreamEvent {
        deserialize_response_event(data, true)
            .expect("valid event")
            .expect("known event type")
    }

    fn minimal_config() -> SamplerConfig {
        SamplerConfig {
            api_key: Some("test-key".to_string()),
            base_url: "https://example.test".to_string(),
            model: "test-model".to_string(),
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            api_backend: ApiBackend::ChatCompletions,
            transport: InferenceTransport::Http,
            auth_scheme: AuthScheme::Bearer,
            capabilities: ProviderCapabilities::default(),
            effective_service_tier: ResolvedServiceTier::default(),
            hosted_multi_agent: Default::default(),
            wire_quirks: WireQuirks::default(),
            extra_headers: IndexMap::new(),
            context_window: 8192,
            force_http1: false,
            max_retries: None,
            stream_tool_calls: false,
            idle_timeout_secs: None,
            reasoning_effort: None,
            origin_client: None,
            client_identifier: None,
            deployment_id: None,
            user_id: None,
            client_version: None,
            attribution_callback: None,
            bearer_resolver: None,
            supports_backend_search: false,
            compactions_remaining: None,
            compaction_at_tokens: None,
            doom_loop_recovery: None,
            header_injector: None,
        }
    }

    /// Verify the serialized shape of StreamingChatRequest matches the
    /// expected wire format: all ChatCompletionRequest fields flattened at
    /// top level, plus `stream: true` and `stream_options.include_usage: true`.
    #[test]
    fn streaming_chat_request_serializes_correctly() {
        let request = ChatCompletionRequest {
            model: Some("test-model".into()),
            messages: vec![ChatRequestMessage::user("hello")],
            temperature: Some(0.7),
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            search_parameters: None,
            response_format: None,
            reasoning_effort: None,
            x_grok_conv_id: None,
            x_grok_req_id: None,
            x_grok_session_id: None,
            x_grok_turn_idx: None,
            x_grok_agent_id: None,
            x_grok_deployment_id: None,
            x_grok_user_id: None,
            trace: None,
        };

        let wrapper = StreamingChatRequest {
            inner: &request,
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        let json: serde_json::Value = serde_json::to_value(&wrapper).unwrap();
        let obj = json.as_object().unwrap();

        assert_eq!(obj.get("stream").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            obj.get("stream_options")
                .and_then(|v| v.get("include_usage"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        assert!(
            obj.get("inner").is_none(),
            "inner field should be flattened"
        );
        assert_eq!(
            obj.get("model").and_then(|v| v.as_str()),
            Some("test-model")
        );
        assert!(obj.get("messages").is_some());
        let temp = obj.get("temperature").and_then(|v| v.as_f64()).unwrap();
        assert!((temp - 0.7).abs() < 0.001, "temperature should be ~0.7");

        assert!(obj.get("max_tokens").is_none());
        assert!(obj.get("tools").is_none());
    }

    #[test]
    fn first_party_xai_url_trust_boundary_rejects_suffix_and_path_attacks() {
        assert!(is_first_party_xai_url("https://api.x.ai/v1"));
        assert!(is_first_party_xai_url("https://x.ai"));
        assert!(is_first_party_xai_url("https://cli-chat-proxy.grok.com/v1"));
        assert!(is_first_party_xai_url(
            "https://cli-chat-proxy.grok.com/v1/responses"
        ));

        assert!(!is_first_party_xai_url("https://api.openai.com/v1"));
        assert!(!is_first_party_xai_url("http://api.x.ai/v1"));
        assert!(!is_first_party_xai_url("https://api.x.ai:444/v1"));
        assert!(!is_first_party_xai_url("https://api.x.ai.evil.example/v1"));
        assert!(!is_first_party_xai_url(
            "https://evil-x.ai.attacker.example/v1"
        ));
        assert!(!is_first_party_xai_url(
            "https://cli-chat-proxy.grok.com.evil.example/v1"
        ));
        assert!(!is_first_party_xai_url(
            "https://cli-chat-proxy.grok.com/v11"
        ));
        assert!(!is_first_party_xai_url("http://cli-chat-proxy.grok.com/v1"));
        assert!(!is_first_party_xai_url(
            "https://cli-chat-proxy.grok.com:444/v1"
        ));
        assert!(!is_first_party_xai_url("https://other.grok.com/v1"));
        assert!(!is_first_party_xai_url("not-a-url"));
    }

    #[test]
    fn non_xai_metadata_ignores_x_grok_headers_but_keeps_models_etag() {
        let mut headers = HeaderMap::new();
        headers.insert("x-grok-context-window", "1048576".parse().unwrap());
        headers.insert("x-grok-max-completion-tokens", "131072".parse().unwrap());
        headers.insert("x-models-etag", "openai-models-v1".parse().unwrap());

        let non_xai = extract_model_metadata(&headers, false).expect("etag is provider-neutral");
        assert_eq!(non_xai.context_window, None);
        assert_eq!(non_xai.max_completion_tokens, None);
        assert_eq!(non_xai.models_etag.as_deref(), Some("openai-models-v1"));

        let xai = extract_model_metadata(&headers, true).expect("xAI metadata is accepted");
        assert_eq!(xai.context_window, Some(1_048_576));
        assert_eq!(xai.max_completion_tokens, Some(131_072));
        assert_eq!(xai.models_etag.as_deref(), Some("openai-models-v1"));
    }

    #[test]
    fn non_xai_chat_wire_strips_xai_only_body_fields() {
        let mut request = ChatCompletionRequest {
            model: Some("gpt-test".to_owned()),
            messages: vec![ChatRequestMessage::assistant(
                "answer",
                "private-xai-model-id",
                Some("private reasoning".to_owned()),
            )],
            search_parameters: Some(xai_grok_sampling_types::SearchParameters {
                mode: Some("on".to_owned()),
                sources: None,
                from_date: None,
                to_date: None,
                return_citations: None,
                max_search_results: None,
            }),
            ..ChatCompletionRequest::new("gpt-test", Vec::new())
        };

        let mut custom_cfg = minimal_config();
        custom_cfg.base_url = "https://api.openai.com/v1".to_owned();
        let custom = SamplingClient::new(custom_cfg).expect("custom client builds");
        custom.strip_non_xai_chat_fields(&mut request);
        let body = serde_json::to_value(&request).expect("chat request serializes");
        assert!(body.get("search_parameters").is_none());
        assert!(body.pointer("/messages/0/model_id").is_none());
        assert!(body.pointer("/messages/0/reasoning_content").is_none());

        let mut xai_request = ChatCompletionRequest {
            model: Some("grok-test".to_owned()),
            messages: vec![ChatRequestMessage::assistant(
                "answer",
                "grok-private-model-id",
                Some("reasoning".to_owned()),
            )],
            search_parameters: Some(xai_grok_sampling_types::SearchParameters {
                mode: Some("on".to_owned()),
                sources: None,
                from_date: None,
                to_date: None,
                return_citations: None,
                max_search_results: None,
            }),
            ..ChatCompletionRequest::new("grok-test", Vec::new())
        };
        let mut xai_cfg = minimal_config();
        xai_cfg.base_url = "https://api.x.ai/v1".to_owned();
        let xai = SamplingClient::new(xai_cfg).expect("xAI client builds");
        xai.strip_non_xai_chat_fields(&mut xai_request);
        let xai_body = serde_json::to_value(&xai_request).expect("xAI request serializes");
        assert_eq!(
            xai_body
                .pointer("/messages/0/model_id")
                .and_then(serde_json::Value::as_str),
            Some("grok-private-model-id")
        );
        assert!(xai_body.get("search_parameters").is_some());
    }

    #[test]
    fn non_streaming_non_xai_conversion_keeps_function_named_x_search() {
        let mut cfg = minimal_config();
        cfg.base_url = "https://api.openai.com/v1".to_owned();
        cfg.api_backend = ApiBackend::Responses;
        let client = SamplingClient::new(cfg).expect("OpenAI client builds");

        let mut request = ConversationRequest::from_items(vec![ConversationItem::user("hello")]);
        request.tools.push(xai_grok_sampling_types::ToolSpec {
            name: "x_search".to_owned(),
            description: Some("provider-neutral function".to_owned()),
            parameters: serde_json::json!({"type": "object"}),
        });
        request.hosted_tools = vec![xai_grok_sampling_types::HostedTool::XSearch { options: None }];
        client.strip_non_xai_hosted_tools(&mut request);

        let wrapper: CreateResponseWrapper = (&request).into();
        let body = serde_json::to_value(&wrapper.inner).expect("Responses request serializes");
        let tools = body["tools"].as_array().expect("function tool remains");
        assert!(tools.iter().any(|tool| {
            tool.get("type").and_then(serde_json::Value::as_str) == Some("function")
                && tool.get("name").and_then(serde_json::Value::as_str) == Some("x_search")
        }));
    }

    #[test]
    fn openai_wire_omits_xai_headers_and_body_extensions() {
        let mut cfg = minimal_config();
        cfg.base_url = "https://api.openai.com/v1".to_owned();
        cfg.api_backend = ApiBackend::Responses;
        cfg.stream_tool_calls = true;
        cfg.doom_loop_recovery = Some(Default::default());
        cfg.client_identifier = Some("should-not-leak".to_owned());
        cfg.client_version = Some("should-not-leak".to_owned());
        cfg.deployment_id = Some("should-not-leak".to_owned());
        cfg.user_id = Some("should-not-leak".to_owned());
        cfg.extra_headers
            .insert("x-grok-extra".to_owned(), "should-not-leak".to_owned());
        cfg.extra_headers
            .insert("x-xai-token-auth".to_owned(), "should-not-leak".to_owned());
        cfg.extra_headers.insert(
            "x-compactions-remaining".to_owned(),
            "should-not-leak".to_owned(),
        );
        cfg.extra_headers
            .insert("x-compaction-at".to_owned(), "should-not-leak".to_owned());

        let client = SamplingClient::new(cfg).expect("client builds");
        assert!(!client.xai_wire_extensions);
        assert!(client.default_headers.contains_key(AUTHORIZATION));
        assert!(client.default_headers.contains_key(CONTENT_TYPE));
        assert!(client.default_headers.contains_key(USER_AGENT));
        assert!(
            client
                .default_headers
                .keys()
                .all(|name| !is_xai_only_header(name.as_str()))
        );

        let tracking = GrokRequestHeaders {
            conv_id: "conv",
            req_id: "req",
            model_id: "gpt-5.6",
            session_id: "session",
            turn_idx: Some("1"),
            agent_id: "agent",
            deployment_id: Some("deployment"),
            user_id: Some("user"),
        };
        let request = tracking
            .apply(
                client.post("https://api.openai.com/v1/responses"),
                client.xai_wire_extensions,
            )
            .build()
            .expect("request builds");
        assert!(
            request
                .headers()
                .keys()
                .all(|name| !is_xai_only_header(name.as_str()))
        );

        let mut body = serde_json::json!({"tools": [{"type": "web_search"}]});
        client.finalize_response_body(
            &mut body,
            None,
            true,
            vec![serde_json::json!({"type": "x_search"})],
        );
        assert!(body.get("stream_tool_calls").is_none());
        assert_eq!(body["tools"], serde_json::json!([{"type": "web_search"}]));
        assert!(
            client
                .xai_wire_extensions
                .then(|| client.defaults.doom_loop_recovery)
                .flatten()
                .is_none()
        );
    }

    #[test]
    fn xai_wire_keeps_tracking_and_opt_in_extensions() {
        let mut cfg = minimal_config();
        cfg.base_url = "https://api.x.ai/v1".to_owned();
        cfg.api_backend = ApiBackend::Responses;
        cfg.stream_tool_calls = true;
        cfg.doom_loop_recovery = Some(Default::default());
        cfg.client_identifier = Some("grok-build-test".to_owned());
        cfg.client_version = Some("1.2.3".to_owned());

        let client = SamplingClient::new(cfg).expect("client builds");
        assert!(client.xai_wire_extensions);
        assert_eq!(
            client
                .default_headers
                .get("x-grok-client-identifier")
                .and_then(|value| value.to_str().ok()),
            Some("grok-build-test")
        );
        assert_eq!(
            client
                .default_headers
                .get("x-grok-client-version")
                .and_then(|value| value.to_str().ok()),
            Some("1.2.3")
        );

        let tracking = GrokRequestHeaders {
            conv_id: "conv",
            req_id: "req",
            model_id: "grok-build",
            session_id: "session",
            turn_idx: Some("1"),
            agent_id: "agent",
            deployment_id: None,
            user_id: None,
        };
        let request = tracking
            .apply(
                client.post("https://api.x.ai/v1/responses"),
                client.xai_wire_extensions,
            )
            .header(DOOM_LOOP_CHECK_HEADER, "true")
            .build()
            .expect("request builds");
        assert_eq!(
            request
                .headers()
                .get("x-grok-conv-id")
                .and_then(|value| value.to_str().ok()),
            Some("conv")
        );
        assert!(request.headers().contains_key(DOOM_LOOP_CHECK_HEADER));

        let mut body = serde_json::json!({"tools": [{"type": "web_search"}]});
        client.finalize_response_body(
            &mut body,
            None,
            true,
            vec![serde_json::json!({"type": "x_search"})],
        );
        assert_eq!(body["stream_tool_calls"], serde_json::json!(true));
        assert_eq!(
            body["tools"],
            serde_json::json!([{"type": "web_search"}, {"type": "x_search"}])
        );
        assert!(client.defaults.doom_loop_recovery.is_some());
    }

    #[test]
    fn responses_reasoning_effort_keeps_xhigh_and_max_distinct_on_wire() {
        let cfg = SamplerConfig {
            api_backend: ApiBackend::Responses,
            reasoning_effort: Some(ReasoningEffort::Max),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client builds");

        for (explicit, expected_effort, expected_wire) in [
            (None, ReasoningEffort::Max, "max"),
            (
                Some(ReasoningEffort::Xhigh),
                ReasoningEffort::Xhigh,
                "xhigh",
            ),
            (Some(ReasoningEffort::Max), ReasoningEffort::Max, "max"),
        ] {
            let mut request = CreateResponseWrapper::default();
            request.reasoning_effort = explicit;
            client
                .apply_response_defaults(&mut request)
                .expect("defaults apply");
            assert_eq!(request.reasoning_effort, Some(expected_effort));
            let mut body = serde_json::to_value(&request.inner).expect("request serializes");
            client.finalize_response_body(&mut body, request.reasoning_effort, false, Vec::new());
            assert_eq!(
                body.pointer("/reasoning/effort")
                    .and_then(serde_json::Value::as_str),
                Some(expected_wire)
            );
        }
    }

    #[test]
    fn extract_retry_after_parses_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(30));
    }

    #[test]
    fn extract_retry_after_caps_at_120() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(120));
    }

    #[test]
    fn extract_retry_after_zero_is_valid() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "0".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(0));
    }

    #[test]
    fn extract_retry_after_ignores_http_date() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Fri, 31 Dec 2025 23:59:59 GMT".parse().unwrap(),
        );
        assert_eq!(extract_retry_after(&headers), None);
    }

    #[test]
    fn extract_retry_after_none_when_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(extract_retry_after(&headers), None);
    }

    #[test]
    fn extracts_all_openai_compatible_rate_limit_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("x-ratelimit-limit-requests", "120"),
            ("x-ratelimit-remaining-requests", "117"),
            ("x-ratelimit-reset-requests", "1.5s"),
            ("x-ratelimit-limit-tokens", "200000"),
            ("x-ratelimit-remaining-tokens", "190000"),
            ("x-ratelimit-reset-tokens", "1m2s"),
            ("x-ratelimit-limit-project-tokens", "500000"),
            ("x-ratelimit-remaining-project-tokens", "450000"),
            ("x-ratelimit-reset-project-tokens", "250ms"),
            ("retry-after", "7"),
        ] {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }

        let limits = extract_rate_limit_metadata(&headers).expect("typed limits");
        assert_eq!(limits.requests.limit, Some(120));
        assert_eq!(limits.requests.remaining, Some(117));
        assert_eq!(limits.requests.reset_after_ms, Some(1_500));
        assert_eq!(limits.tokens.limit, Some(200_000));
        assert_eq!(limits.tokens.remaining, Some(190_000));
        assert_eq!(limits.tokens.reset_after_ms, Some(62_000));
        assert_eq!(limits.project_tokens.limit, Some(500_000));
        assert_eq!(limits.project_tokens.remaining, Some(450_000));
        assert_eq!(limits.project_tokens.reset_after_ms, Some(250));
        assert_eq!(limits.retry_after_ms, Some(7_000));
    }

    #[test]
    fn rate_limit_headers_are_provider_neutral_model_metadata() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-limit-requests", "10".parse().unwrap());
        let metadata = extract_model_metadata(&headers, false).expect("rate metadata");
        assert_eq!(
            metadata.rate_limits.expect("limits").requests.limit,
            Some(10)
        );
    }

    #[test]
    fn extract_should_retry_true() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "true".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(true));
    }

    #[test]
    fn extract_should_retry_true_case_insensitive() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "TRUE".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(true));
    }

    #[test]
    fn extract_should_retry_false() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "false".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(false));
    }

    #[test]
    fn extract_should_retry_unknown_value_is_none() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "banana".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), None);
    }

    #[test]
    fn extract_should_retry_absent_is_none() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(extract_should_retry(&headers), None);
    }

    #[test]
    fn new_with_minimal_config_succeeds() {
        let client = SamplingClient::new(minimal_config()).expect("client should construct");
        assert_eq!(client.api_backend(), ApiBackend::ChatCompletions);
    }

    #[test]
    fn new_applies_extra_headers() {
        let mut cfg = minimal_config();
        cfg.extra_headers
            .insert("x-test-header".to_string(), "test-value".to_string());
        cfg.extra_headers
            .insert("x-XAI-token-auth".to_string(), "xai-grok-cli".to_string());
        let _client = SamplingClient::new(cfg).expect("client with extra headers should construct");
    }

    #[test]
    fn messages_plus_anthropic_api_key_uses_x_api_key_and_not_authorization() {
        let cfg = SamplerConfig {
            api_key: Some("anthropic-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert!(
            client
                .default_headers
                .get(HeaderName::from_static("x-api-key"))
                .is_some()
        );
        assert!(client.default_headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn messages_plus_bearer_uses_authorization_and_not_x_api_key() {
        let cfg = SamplerConfig {
            api_key: Some("bearer-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::Bearer,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert!(client.default_headers.get(AUTHORIZATION).is_some());
        assert!(
            client
                .default_headers
                .get(HeaderName::from_static("x-api-key"))
                .is_none()
        );
    }

    // Regression: a past change dropped User-Agent from sampling requests.
    #[test]
    fn sampling_client_always_has_user_agent() {
        let client = SamplingClient::new(minimal_config()).expect("build");
        assert!(client.default_headers.contains_key(USER_AGENT));
    }

    // Regression: a past change dropped HeaderInjector (traceparent) from sampling requests.
    #[test]
    fn header_injector_is_called_in_post() {
        #[derive(Debug)]
        struct TestInjector;
        impl crate::config::HeaderInjector for TestInjector {
            fn inject(&self, headers: &mut HeaderMap) {
                headers.insert(
                    HeaderName::from_static("traceparent"),
                    HeaderValue::from_static("00-test-trace-id-00"),
                );
                headers.insert(
                    HeaderName::from_static("x-grok-injected"),
                    HeaderValue::from_static("must-not-leak"),
                );
                headers.insert(
                    HeaderName::from_static("x-xai-token-auth"),
                    HeaderValue::from_static("must-not-leak"),
                );
                headers.insert(
                    HeaderName::from_static("x-compactions-remaining"),
                    HeaderValue::from_static("must-not-leak"),
                );
            }
        }

        let mut config = minimal_config();
        config.header_injector = Some(std::sync::Arc::new(TestInjector));
        let client = SamplingClient::new(config).expect("build");
        let req = client
            .post("http://localhost/test")
            .build()
            .expect("build request");
        assert!(
            req.headers().contains_key("traceparent"),
            "HeaderInjector should inject traceparent into post() requests"
        );
        assert!(
            req.headers()
                .keys()
                .all(|name| !is_xai_only_header(name.as_str())),
            "HeaderInjector cannot bypass the non-xAI denylist"
        );
    }

    #[test]
    fn user_agent_includes_origin_and_agent_product() {
        let origin = OriginClientInfo {
            product: "my-client".to_string(),
            version: Some("1.2.3".to_string()),
        };
        let ua = user_agent_string_for(&origin);
        assert!(ua.contains("my-client/1.2.3"));
        assert!(ua.contains(AGENT_PRODUCT));
    }

    #[test]
    fn user_agent_omits_origin_version_when_absent() {
        let origin = OriginClientInfo {
            product: "my-client".to_string(),
            version: None,
        };
        let ua = user_agent_string_for(&origin);
        // No slash between product and the grok-shell agent product.
        assert!(ua.starts_with("my-client grok-shell/"));
    }

    #[test]
    fn user_agent_collapses_when_origin_matches_agent() {
        let agent_version = xai_grok_version::VERSION.to_string();
        let origin = OriginClientInfo {
            product: AGENT_PRODUCT.to_string(),
            version: Some(agent_version.clone()),
        };
        let ua = user_agent_string_for(&origin);
        // Single product/version slot when the origin and agent match.
        assert!(ua.starts_with(&format!("{}/{}", AGENT_PRODUCT, agent_version)));
    }

    /// Counts callbacks for assertions in the tests below.
    #[derive(Default, Debug)]
    struct CountingCallback {
        invocations: std::sync::Mutex<Vec<(crate::attribution::SamplingConsumer, Option<String>)>>,
    }

    #[derive(Debug)]
    struct StaticBearerResolver(&'static str);

    impl crate::config::BearerResolver for StaticBearerResolver {
        fn current_bearer(&self) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    impl crate::attribution::Auth401AttributionCallback for CountingCallback {
        fn record_401(
            &self,
            consumer: crate::attribution::SamplingConsumer,
            sent_bearer: Option<&str>,
        ) {
            self.invocations
                .lock()
                .unwrap()
                .push((consumer, sent_bearer.map(|s| s.to_string())));
        }
    }

    /// `extract_sent_bearer` strips the `"Bearer "` prefix off
    /// `Authorization` for OpenAI-completions backends and truncates the
    /// remaining bearer to the cross-crate prefix length.
    #[test]
    fn extract_sent_bearer_strips_bearer_prefix_for_openai_compat() {
        let cfg = SamplerConfig {
            api_key: Some("test-bearer-1234567890".to_string()),
            api_backend: ApiBackend::ChatCompletions,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let bearer = client.extract_sent_bearer();
        // Bearer is truncated at the crate boundary -- callers
        // downstream of this method only ever see the prefix.
        assert_eq!(bearer.as_deref(), Some("test-bearer-"));
        assert_eq!(
            bearer.as_deref().map(str::len),
            Some(crate::attribution::SENT_BEARER_PREFIX_LEN),
        );
    }

    /// `extract_sent_bearer` reads `x-api-key` for Anthropic Messages API
    /// and truncates the value to the cross-crate prefix length.
    #[test]
    fn extract_sent_bearer_reads_x_api_key_for_messages() {
        let cfg = SamplerConfig {
            api_key: Some("anthropic-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let bearer = client.extract_sent_bearer();
        assert_eq!(bearer.as_deref(), Some("anthropic-ke"));
        assert_eq!(
            bearer.as_deref().map(str::len),
            Some(crate::attribution::SENT_BEARER_PREFIX_LEN),
        );
    }

    /// `extract_sent_bearer` returns `None` when no auth header is set.
    #[test]
    fn extract_sent_bearer_returns_none_when_no_header() {
        let cfg = SamplerConfig {
            api_key: None,
            api_backend: ApiBackend::ChatCompletions,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert!(client.extract_sent_bearer().is_none());
    }

    #[test]
    fn live_bearer_resolver_uses_authorization_for_messages_plus_bearer() {
        let cfg = SamplerConfig {
            api_key: Some("stale-bearer".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::Bearer,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-bearer"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let request = client
            .post("https://example.test/v1/messages")
            .build()
            .expect("request should build");
        let auth = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        assert_eq!(auth, Some("Bearer fresh-bearer"));
        assert!(request.headers().get("x-api-key").is_none());
    }

    /// Regression: when `api_key` (which seeds `default_headers` with an
    /// `Authorization: Bearer ...`) AND a `bearer_resolver` are both set,
    /// `post()` must produce **exactly one** `Authorization` header on the
    /// wire. The pre-fix code used `RequestBuilder::header(AUTHORIZATION, ...)`
    /// which appends rather than replaces, causing two identical
    /// `Authorization` headers and a 400 from cli-chat-proxy.
    #[test]
    fn post_emits_single_authorization_with_api_key_and_bearer_resolver() {
        let cfg = SamplerConfig {
            api_key: Some("stale-bearer".to_string()),
            api_backend: ApiBackend::Responses,
            auth_scheme: AuthScheme::Bearer,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-bearer"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let request = client
            .post("https://example.test/v1/responses")
            .build()
            .expect("request should build");
        let auth_count = request.headers().get_all(AUTHORIZATION).iter().count();
        assert_eq!(
            auth_count, 1,
            "expected exactly one Authorization header, got {auth_count}"
        );
        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer fresh-bearer"),
        );
    }

    #[test]
    fn live_bearer_resolver_uses_x_api_key_for_messages_plus_anthropic_api_key() {
        let cfg = SamplerConfig {
            api_key: Some("stale-anthropic".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-anthropic"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let request = client
            .post("https://example.test/v1/messages")
            .build()
            .expect("request should build");
        let api_key = request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        assert_eq!(api_key, Some("fresh-anthropic"));
        assert!(request.headers().get(AUTHORIZATION).is_none());
    }

    /// Bearers shorter than the prefix length pass through unchanged.
    /// Defensive against the truncation logic inadvertently widening
    /// short bearers (no panics, no zero-padding).
    #[test]
    fn extract_sent_bearer_short_bearer_passes_through_unchanged() {
        let cfg = SamplerConfig {
            api_key: Some("abc".to_string()),
            api_backend: ApiBackend::ChatCompletions,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert_eq!(client.extract_sent_bearer().as_deref(), Some("abc"));
    }

    /// `record_401_attribution` invokes the wired callback with the
    /// expected `consumer` and the truncated bearer prefix that the
    /// wire would carry. The key assertion is that the callback
    /// receives the prefix only -- the full bearer never crosses the
    /// crate boundary.
    #[test]
    fn record_401_attribution_invokes_callback_with_extracted_bearer() {
        let cb = std::sync::Arc::new(CountingCallback::default());
        let cb_dyn: crate::attribution::SharedAttributionCallback = cb.clone();
        let cfg = SamplerConfig {
            api_key: Some("the-bearer-1234567890-extra-tail".to_string()),
            base_url: "https://api.x.ai/v1".to_string(),
            api_backend: ApiBackend::ChatCompletions,
            attribution_callback: Some(cb_dyn),
            bearer_resolver: None,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        client.record_401_attribution(crate::attribution::SamplingConsumer::ChatCompletionsStream);
        let calls = cb.invocations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0,
            crate::attribution::SamplingConsumer::ChatCompletionsStream
        );
        // Prefix-only -- the `extra-tail` portion of the bearer is
        // dropped by `extract_sent_bearer` before the callback fires.
        assert_eq!(calls[0].1.as_deref(), Some("the-bearer-1"));
        assert_eq!(
            calls[0].1.as_deref().map(str::len),
            Some(crate::attribution::SENT_BEARER_PREFIX_LEN),
        );
    }

    #[test]
    fn record_401_attribution_never_exposes_non_xai_key_prefix() {
        let cb = std::sync::Arc::new(CountingCallback::default());
        let cb_dyn: crate::attribution::SharedAttributionCallback = cb.clone();
        let cfg = SamplerConfig {
            api_key: Some("sk-openai-secret-value".to_string()),
            base_url: "https://api.openai.com/v1".to_string(),
            api_backend: ApiBackend::Responses,
            attribution_callback: Some(cb_dyn),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        client.record_401_attribution(crate::attribution::SamplingConsumer::ResponsesStream);
        assert!(
            cb.invocations.lock().unwrap().is_empty(),
            "non-xAI credentials must never reach attribution"
        );
    }

    /// Regression test: when a bearer_resolver is wired, `post()` must
    /// *replace* the Authorization header from `default_headers`, not
    /// append a second one. Duplicate Authorization headers cause
    /// Cloudflare to return 400 Bad Request.
    #[test]
    fn bearer_resolver_replaces_authorization_header() {
        #[derive(Debug)]
        struct StaticResolver(String);
        impl crate::config::BearerResolver for StaticResolver {
            fn current_bearer(&self) -> Option<String> {
                Some(self.0.clone())
            }
        }

        let resolver: crate::config::SharedBearerResolver =
            std::sync::Arc::new(StaticResolver("fresh-token".to_string()));
        let cfg = SamplerConfig {
            api_key: Some("stale-token".to_string()),
            api_backend: ApiBackend::Responses,
            bearer_resolver: Some(resolver),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");

        // Build a request to inspect the final headers.
        let builder = client.post("https://example.test/v1/responses");
        let request = builder.body("").build().expect("request should build");

        let auth_values: Vec<_> = request.headers().get_all(AUTHORIZATION).iter().collect();
        assert_eq!(
            auth_values.len(),
            1,
            "expected exactly one Authorization header, got {}: {:?}",
            auth_values.len(),
            auth_values
        );
        assert_eq!(
            auth_values[0].to_str().unwrap(),
            "Bearer fresh-token",
            "Authorization header should contain the resolver's fresh token"
        );
    }

    /// `record_401_attribution` is a no-op when `attribution_callback`
    /// is `None` (the BYOK / sampler-only path). The previous tests
    /// in this module construct clients without a callback and rely
    /// on this property holding.
    #[test]
    fn record_401_attribution_is_noop_without_callback() {
        let cfg = SamplerConfig {
            api_key: Some("bearer".to_string()),
            api_backend: ApiBackend::ChatCompletions,
            attribution_callback: None,
            bearer_resolver: None,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        // Must not panic.
        client.record_401_attribution(crate::attribution::SamplingConsumer::ChatCompletions);
    }

    /// `response.completed` carrying
    /// `usage.context_details.{input_tokens, output_tokens}` rewrites
    /// `usage.total_tokens` in place to the live context length
    /// (`ctx.input + ctx.output`). Billing fields stay on the wire's
    /// cumulative values.
    #[test]
    fn deserialize_response_event_overrides_total_tokens_from_context_details() {
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 6003,
                    "input_tokens_details": { "cached_tokens": 1984 },
                    "output_tokens": 711,
                    "output_tokens_details": { "reasoning_tokens": 388 },
                    "total_tokens": 6714,
                    "context_details": {
                        "input_tokens": 5022,
                        "output_tokens": 571
                    }
                }
            }
        }"#;
        let event = known_xai_response_event(sse);
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        // Billing fields stay cumulative — unchanged by context_details.
        assert_eq!(usage.input_tokens, 6003);
        assert_eq!(usage.output_tokens, 711);
        assert_eq!(usage.input_tokens_details.cached_tokens, 1984);
        assert_eq!(usage.output_tokens_details.reasoning_tokens, 388);
        // total_tokens rewritten to ctx.input + ctx.output (5022 + 571).
        // NOT the wire's cumulative total (6714).
        assert_eq!(usage.total_tokens, 5_593);
    }

    #[test]
    fn deserialize_response_event_stashes_cost_in_metadata() {
        let make = |ticks: i64| {
            format!(
                r#"{{
                "type": "response.completed",
                "sequence_number": 0,
                "response": {{
                    "id": "resp_1", "object": "response", "created_at": 0,
                    "model": "grok-build", "status": "completed", "output": [],
                    "usage": {{
                        "input_tokens": 10,
                        "input_tokens_details": {{ "cached_tokens": 0 }},
                        "output_tokens": 5,
                        "output_tokens_details": {{ "reasoning_tokens": 0 }},
                        "total_tokens": 15,
                        "cost_in_usd_ticks": {ticks}
                    }}
                }}
            }}"#
            )
        };

        let event = known_xai_response_event(&make(78));
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        assert_eq!(
            e.response
                .metadata
                .as_ref()
                .and_then(|m| m.get(COST_USD_TICKS_METADATA_KEY))
                .map(String::as_str),
            Some("78")
        );

        // The REST mapper backfills 0 for unbilled requests: no stash.
        let event = known_xai_response_event(&make(0));
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        assert!(e.response.metadata.is_none());
    }

    #[test]
    fn non_xai_terminal_event_ignores_xai_context_and_cost_overrides() {
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "gpt-test",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 5,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 15,
                    "cost_in_usd_ticks": 78,
                    "context_details": {
                        "input_tokens": 3,
                        "output_tokens": 2
                    }
                }
            }
        }"#;

        let event = deserialize_response_event(sse, false)
            .expect("known OpenAI event deserializes")
            .expect("known event is retained");
        let rs::ResponseStreamEvent::ResponseCompleted(event) = event else {
            panic!("expected ResponseCompleted");
        };
        assert_eq!(
            event.response.usage.expect("usage present").total_tokens,
            15
        );
        assert!(
            event
                .response
                .metadata
                .as_ref()
                .is_none_or(|metadata| !metadata.contains_key(COST_USD_TICKS_METADATA_KEY))
        );
    }

    #[test]
    fn deserialize_response_event_total_tokens_unchanged_when_context_details_absent() {
        // Older / non-Responses backends omit `context_details`.
        // `total_tokens` passes through from the wire unchanged.
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10000,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 100,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 10100
                }
            }
        }"#;
        let event = known_xai_response_event(sse);
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        assert_eq!(usage.total_tokens, 10_100);
    }

    #[test]
    fn deserialize_response_event_total_tokens_unchanged_when_context_details_partial() {
        // Defensive: if the backend ever ships only one of the two
        // context_details fields, we don't have a complete picture of
        // the live context size, so leave `total_tokens` on the wire's
        // cumulative value instead of guessing (treating the missing
        // half as 0 would silently under-report).
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 6003,
                    "input_tokens_details": { "cached_tokens": 1984 },
                    "output_tokens": 711,
                    "output_tokens_details": { "reasoning_tokens": 388 },
                    "total_tokens": 6714,
                    "context_details": {
                        "input_tokens": 5022
                    }
                }
            }
        }"#;
        let event = known_xai_response_event(sse);
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        assert_eq!(usage.total_tokens, 6_714);
    }

    #[test]
    fn deserialize_response_event_ignores_context_details_on_non_terminal_events() {
        // Non-terminal events don't carry final usage; even if the backend ever
        // echoed `context_details` on one, we don't touch it.
        let sse = r#"{
            "type": "response.output_text.delta",
            "sequence_number": 0,
            "item_id": "item-1",
            "output_index": 0,
            "content_index": 0,
            "delta": "hello",
            "logprobs": []
        }"#;
        let event = known_xai_response_event(sse);
        assert!(matches!(
            event,
            rs::ResponseStreamEvent::ResponseOutputTextDelta(_)
        ));
    }

    #[test]
    fn deserialize_response_event_ignores_well_formed_unknown_event_type() {
        let event = deserialize_response_event(
            r#"{"type":"response.future_capability.delta","sequence_number":1,"delta":"private content"}"#,
            false,
        )
        .expect("well-formed future event must not fail the stream");
        assert!(event.is_none());
    }

    #[test]
    fn deserialize_response_event_rejects_malformed_known_and_terminal_events() {
        let malformed_delta = r#"{"type":"response.output_text.delta","sequence_number":1}"#;
        assert!(deserialize_response_event(malformed_delta, false).is_err());

        let malformed_terminal =
            r#"{"type":"response.completed","sequence_number":2,"response":{}}"#;
        assert!(deserialize_response_event(malformed_terminal, false).is_err());

        let malformed_max_terminal = r#"{
            "type": "response.completed",
            "sequence_number": 3,
            "response": {"reasoning": {"effort": "max"}}
        }"#;
        assert!(deserialize_response_event(malformed_max_terminal, false).is_err());
        assert!(deserialize_response_body(br#"{"reasoning":{"effort":"max"}}"#).is_err());

        assert!(deserialize_response_event("not json", false).is_err());
    }

    #[test]
    fn streaming_response_preserves_native_max_through_sdk_staging() {
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_max_stream",
                "object": "response",
                "created_at": 0,
                "model": "gpt-5.6",
                "status": "completed",
                "output": [],
                "reasoning": {"effort": "max", "summary": "auto"}
            }
        }"#;

        let event = deserialize_response_event(sse, false)
            .expect("OpenAI max event must deserialize")
            .expect("response.completed is known");
        let rs::ResponseStreamEvent::ResponseCompleted(event) = event else {
            panic!("expected ResponseCompleted");
        };
        assert_eq!(
            event
                .response
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.clone()),
            Some(rs::ReasoningEffort::Max),
            "the SDK must preserve its native max reasoning-effort value"
        );
        assert_eq!(
            event
                .response
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get(CANONICAL_REASONING_EFFORT_METADATA_KEY))
                .map(String::as_str),
            None,
            "native max no longer needs compatibility metadata"
        );

        let items = xai_grok_sampling_types::response_to_conversation_items(event.response);
        let assistant = items
            .iter()
            .find_map(|item| match item {
                xai_grok_sampling_types::ConversationItem::Assistant(assistant) => Some(assistant),
                _ => None,
            })
            .expect("response conversion emits an assistant item");
        assert_eq!(assistant.reasoning_effort, Some(ReasoningEffort::Max));
    }

    #[test]
    fn non_streaming_response_preserves_native_max_through_sdk_staging() {
        let body = br#"{
            "id": "resp_max_body",
            "object": "response",
            "created_at": 0,
            "model": "gpt-5.6",
            "status": "completed",
            "output": [],
            "reasoning": {"effort": "max", "summary": "auto"}
        }"#;

        let response = deserialize_response_body(body).expect("OpenAI max body must deserialize");
        assert_eq!(
            response
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.clone()),
            Some(rs::ReasoningEffort::Max),
            "the SDK must preserve its native max reasoning-effort value"
        );
        assert_eq!(
            response
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get(CANONICAL_REASONING_EFFORT_METADATA_KEY))
                .map(String::as_str),
            None,
            "native max no longer needs compatibility metadata"
        );

        let items = xai_grok_sampling_types::response_to_conversation_items(response);
        let assistant = items
            .iter()
            .find_map(|item| match item {
                xai_grok_sampling_types::ConversationItem::Assistant(assistant) => Some(assistant),
                _ => None,
            })
            .expect("response conversion emits an assistant item");
        assert_eq!(assistant.reasoning_effort, Some(ReasoningEffort::Max));
    }

    #[test]
    fn unknown_tool_sanitizer_is_limited_to_verified_xai_streams() {
        let terminal_with_x_search = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "tools": [{"type": "x_search"}]
            }
        }"#;
        assert!(
            deserialize_response_event(terminal_with_x_search, false).is_err(),
            "custom/OpenAI streams must remain strict for malformed known events"
        );
        assert!(matches!(
            deserialize_response_event(terminal_with_x_search, true),
            Ok(Some(rs::ResponseStreamEvent::ResponseCompleted(_)))
        ));
    }
}
