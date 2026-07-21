//! Native Apple Foundation Models transport over a framed stdio helper.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use async_stream::stream;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use xai_grok_sampling_types::{
    AssistantItem, ContentPart, ConversationItem, ConversationRequest, ConversationResponse,
    SamplingError, StopReason,
};

use crate::events::{SamplingChannel, SamplingErrorInfo, SamplingEvent};
use crate::metrics::InferenceLatencyStats;
use crate::types::RequestId;

const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 4096;
const HELPER_ENV: &str = "BANDICOT_APPLE_FOUNDATION_MODELS_HELPER";
const HELPER_NAME: &str = "bandicot-apple-foundation-models";

#[derive(Debug, Serialize)]
struct BridgeRequest {
    protocol_version: u32,
    operation: &'static str,
    model: String,
    messages: Vec<BridgeMessage>,
    temperature: Option<f32>,
    maximum_response_tokens: Option<u32>,
    json_schema: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BridgeMessage {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<BridgeToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct BridgeToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BridgeFrame {
    Available,
    Snapshot { text: String },
    Completed { text: String },
    Error { code: String, message: String },
}

#[derive(Default)]
struct SnapshotNormalizer {
    previous: String,
}

impl SnapshotNormalizer {
    fn push(&mut self, snapshot: String) -> Result<Option<String>, SamplingError> {
        if snapshot == self.previous {
            return Ok(None);
        }
        let Some(delta) = snapshot.strip_prefix(&self.previous) else {
            return Err(native_error(
                "non_monotonic_stream",
                "Foundation Models replaced already-streamed text",
            ));
        };
        let delta = delta.to_owned();
        self.previous = snapshot;
        Ok((!delta.is_empty()).then_some(delta))
    }
}

pub fn stream_conversation(
    request: ConversationRequest,
    default_model: String,
    default_temperature: Option<f32>,
    default_max_tokens: Option<u32>,
    request_id: RequestId,
    captured_error: Arc<Mutex<Option<SamplingError>>>,
) -> BoxStream<'static, SamplingEvent> {
    stream! {
        let start = Instant::now();
        let mut timestamps = Vec::new();
        let bridge_request = match map_request(request, default_model, default_temperature, default_max_tokens) {
            Ok(request) => request,
            Err(error) => {
                yield failed(&request_id, error, &captured_error);
                return;
            }
        };
        let response_model = bridge_request.model.clone();
        let mut frames = match spawn_frames(bridge_request).await {
            Ok(frames) => frames,
            Err(error) => {
                yield failed(&request_id, error, &captured_error);
                return;
            }
        };
        yield SamplingEvent::StreamStarted {
            request_id: request_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };
        let mut normalizer = SnapshotNormalizer::default();
        let mut chunk_index = 0u64;
        let mut first = true;
        while let Some(frame) = frames.next().await {
            match frame {
                Ok(BridgeFrame::Available) => {}
                Ok(BridgeFrame::Snapshot { text }) => match normalizer.push(text) {
                    Ok(Some(delta)) => {
                        timestamps.push(Instant::now());
                        if first {
                            first = false;
                            yield SamplingEvent::FirstToken { request_id: request_id.clone() };
                        }
                        yield SamplingEvent::ChannelToken {
                            request_id: request_id.clone(),
                            channel: SamplingChannel::Text,
                            text: delta,
                            chunk_index,
                        };
                        chunk_index += 1;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        yield failed(&request_id, error, &captured_error);
                        return;
                    }
                },
                Ok(BridgeFrame::Completed { text }) => {
                    match normalizer.push(text) {
                        Ok(Some(delta)) => {
                            timestamps.push(Instant::now());
                            if first {
                                yield SamplingEvent::FirstToken { request_id: request_id.clone() };
                            }
                            yield SamplingEvent::ChannelToken {
                                request_id: request_id.clone(),
                                channel: SamplingChannel::Text,
                                text: delta,
                                chunk_index,
                            };
                            chunk_index += 1;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            yield failed(&request_id, error, &captured_error);
                            return;
                        }
                    }
                    let metrics = InferenceLatencyStats::from_timestamps(start, &timestamps, Instant::now());
                    let response = ConversationResponse {
                        items: vec![ConversationItem::Assistant(AssistantItem {
                            content: Arc::<str>::from(normalizer.previous),
                            tool_calls: Vec::new(),
                            model_id: Some(response_model),
                            model_fingerprint: None,
                            reasoning_effort: None,
                        })],
                        stop_reason: Some(StopReason::Stop),
                        usage: None,
                        cost_usd_ticks: None,
                        message_chunks_emitted: chunk_index,
                        doom_loop_signals: Vec::new(),
                        stop_message: None,
                    };
                    yield SamplingEvent::Completed {
                        request_id: request_id.clone(),
                        response: Box::new(response),
                        metrics,
                    };
                    return;
                }
                Ok(BridgeFrame::Error { code, message }) => {
                    yield failed(&request_id, native_error(&code, &message), &captured_error);
                    return;
                }
                Err(error) => {
                    yield failed(&request_id, error, &captured_error);
                    return;
                }
            }
        }
        yield failed(&request_id, native_error("unexpected_eof", "Foundation Models helper exited before completion"), &captured_error);
    }
    .boxed()
}

fn map_request(
    request: ConversationRequest,
    default_model: String,
    default_temperature: Option<f32>,
    default_max_tokens: Option<u32>,
) -> Result<BridgeRequest, SamplingError> {
    let has_images = request.items.iter().any(|item| match item {
        ConversationItem::User(user) => user
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. })),
        ConversationItem::ToolResult(result) => !result.images.is_empty(),
        _ => false,
    });
    if has_images {
        return Err(native_error(
            "unsupported_capability",
            "Apple Foundation Models transport does not support image input",
        ));
    }
    let messages = request
        .items
        .into_iter()
        .filter_map(|item| match item {
            ConversationItem::System(system) => Some(BridgeMessage {
                role: "system",
                content: system.content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            }),
            ConversationItem::User(user) => Some(BridgeMessage {
                role: "user",
                content: user
                    .content
                    .into_iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.to_string()),
                        ContentPart::Image { .. } => unreachable!("images rejected above"),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                tool_calls: Vec::new(),
                tool_call_id: None,
            }),
            ConversationItem::Assistant(assistant) => Some(BridgeMessage {
                role: "assistant",
                content: assistant.content.to_string(),
                tool_calls: assistant
                    .tool_calls
                    .into_iter()
                    .map(|call| BridgeToolCall {
                        id: call.id.to_string(),
                        name: call.name,
                        arguments: call.arguments.to_string(),
                    })
                    .collect(),
                tool_call_id: None,
            }),
            ConversationItem::ToolResult(result) => Some(BridgeMessage {
                role: "tool_result",
                content: result.content.to_string(),
                tool_calls: Vec::new(),
                tool_call_id: Some(result.tool_call_id),
            }),
            ConversationItem::BackendToolCall(call) => Some(BridgeMessage {
                role: "user",
                content: call.text_summary(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            }),
            ConversationItem::Reasoning(_) => None,
        })
        .collect();
    Ok(BridgeRequest {
        protocol_version: PROTOCOL_VERSION,
        operation: "generate",
        model: request.model.unwrap_or(default_model),
        messages,
        temperature: request.temperature.or(default_temperature),
        maximum_response_tokens: request.max_output_tokens.or(default_max_tokens),
        json_schema: request.json_schema,
    })
}

async fn spawn_frames(
    request: BridgeRequest,
) -> Result<BoxStream<'static, Result<BridgeFrame, SamplingError>>, SamplingError> {
    if !cfg!(target_os = "macos") {
        return Err(native_error(
            "unsupported_platform",
            "Apple Foundation Models requires macOS 26 or later",
        ));
    }
    let helper = helper_path()?;
    let mut child = Command::new(&helper)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            native_error(
                "helper_spawn_failed",
                &format!("{}: {error}", helper.display()),
            )
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| native_error("helper_io", "helper stdin was unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| native_error("helper_io", "helper stdout was unavailable"))?;
    write_frame(&mut stdin, &request).await?;
    stdin
        .shutdown()
        .await
        .map_err(|error| native_error("helper_io", &error.to_string()))?;
    Ok(stream! {
        let mut stdout = stdout;
        let _child = child;
        loop {
            match read_frame::<_, BridgeFrame>(&mut stdout).await {
                Ok(Some(frame)) => yield Ok(frame),
                Ok(None) => break,
                Err(error) => {
                    yield Err(error);
                    break;
                }
            }
        }
    }
    .boxed())
}

fn helper_path() -> Result<PathBuf, SamplingError> {
    if let Some(path) = std::env::var_os(HELPER_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let current =
        std::env::current_exe().map_err(|error| native_error("helper_path", &error.to_string()))?;
    Ok(current.with_file_name(HELPER_NAME))
}

async fn write_frame<W: AsyncWriteExt + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), SamplingError> {
    let payload = serde_json::to_vec(value).map_err(SamplingError::Serialization)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(native_error(
            "frame_too_large",
            "Foundation Models request exceeded the 8 MiB protocol limit",
        ));
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(|error| native_error("helper_io", &error.to_string()))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|error| native_error("helper_io", &error.to_string()))?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> Result<Option<T>, SamplingError> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(native_error("helper_io", &error.to_string())),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(native_error(
            "invalid_frame",
            "Foundation Models helper sent an invalid frame length",
        ));
    }
    let mut payload = vec![0; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|error| native_error("helper_io", &error.to_string()))?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(SamplingError::Serialization)
}

fn native_error(code: &str, message: &str) -> SamplingError {
    let retryable = matches!(
        code,
        "rate_limited"
            | "concurrent_requests"
            | "assets_unavailable"
            | "model_not_ready"
            | "helper_io"
            | "unexpected_eof"
    );
    SamplingError::NativeTransport {
        code: bounded(code),
        message: bounded(message),
        retryable,
    }
}

fn bounded(value: &str) -> String {
    value.chars().take(MAX_ERROR_CHARS).collect()
}

fn failed(
    request_id: &RequestId,
    error: SamplingError,
    captured: &Arc<Mutex<Option<SamplingError>>>,
) -> SamplingEvent {
    if let Ok(mut slot) = captured.lock() {
        *slot = Some(crate::retry::clone_error(&error));
    }
    SamplingEvent::Failed {
        request_id: request_id.clone(),
        error: SamplingErrorInfo::from(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn cumulative_snapshots_become_nonduplicated_deltas() {
        let mut normalizer = SnapshotNormalizer::default();
        assert_eq!(normalizer.push("Hel".into()).unwrap(), Some("Hel".into()));
        assert_eq!(normalizer.push("Hello".into()).unwrap(), Some("lo".into()));
        assert_eq!(normalizer.push("Hello".into()).unwrap(), None);
        assert!(normalizer.push("Help".into()).is_err());
    }

    #[test]
    fn deterministic_native_failures_are_not_retried() {
        assert!(!native_error("unsupported_platform", "no").is_retryable());
        assert!(!native_error("model_unavailable", "no").is_retryable());
        assert!(native_error("rate_limited", "later").is_retryable());
    }

    #[tokio::test]
    async fn framed_mock_protocol_round_trips_and_bounds_frames() {
        let (mut writer, mut reader) = duplex(1024);
        let write = tokio::spawn(async move {
            write_frame(
                &mut writer,
                &serde_json::json!({"type":"snapshot","text":"hi"}),
            )
            .await
            .unwrap();
        });
        let frame: BridgeFrame = read_frame(&mut reader).await.unwrap().unwrap();
        assert!(matches!(frame, BridgeFrame::Snapshot { text } if text == "hi"));
        write.await.unwrap();

        let (mut writer, mut reader) = duplex(16);
        writer
            .write_all(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(read_frame::<_, BridgeFrame>(&mut reader).await.is_err());
    }
}
