//! Optional Codex app-server JSON-RPC adapter for AgentGraph workers.
//!
//! This backend is deliberately independent from the core scheduler and is
//! disabled by default. It never selects a broader sandbox than read-only and
//! fails closed when the executable or protocol response is unavailable.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::oneshot;

use super::types::{AGENTGRAPH_SCHEMA_VERSION, NodeOutput, NodeStatus, UsageAccounting};
use super::worker::{
    WorkerBackend, WorkerCompletion, WorkerError, WorkerHandle, WorkerRequest, graph_worker_id,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexAppServerConfig {
    pub enabled: bool,
    pub executable: PathBuf,
    pub args: Vec<String>,
}

impl Default for CodexAppServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: PathBuf::from("codex"),
            args: vec!["app-server".to_string()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWorkerOptions {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurnResult {
    pub thread_id: String,
    pub turn_id: String,
    pub output: Value,
    pub notifications: Vec<Value>,
}

struct ActiveCodexWorker {
    request: WorkerRequest,
    cancel: Option<oneshot::Sender<()>>,
    result: oneshot::Receiver<WorkerCompletion>,
}

/// Configuration-gated AgentGraph backend backed by one app-server process per
/// active worker. The scheduler remains usable without constructing this type.
pub struct CodexAppServerWorkerBackend {
    config: CodexAppServerConfig,
    options: CodexWorkerOptions,
    active: BTreeMap<String, ActiveCodexWorker>,
}

impl std::fmt::Debug for CodexAppServerWorkerBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexAppServerWorkerBackend")
            .field("config", &self.config)
            .field("options", &self.options)
            .field("active_count", &self.active.len())
            .finish()
    }
}

impl CodexAppServerWorkerBackend {
    pub fn new(
        config: CodexAppServerConfig,
        options: CodexWorkerOptions,
    ) -> Result<Self, CodexAppServerError> {
        codex_app_server_capability(&config)?;
        Ok(Self {
            config,
            options,
            active: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Error)]
pub enum CodexAppServerError {
    #[error("Codex app-server backend is disabled")]
    Disabled,
    #[error("Codex executable `{0}` is not available")]
    ExecutableUnavailable(String),
    #[error("failed to start Codex app-server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Codex app-server stdin/stdout was not available")]
    MissingPipe,
    #[error("Codex app-server I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Codex app-server exited before replying")]
    UnexpectedEof,
    #[error("Codex app-server returned invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("Codex app-server rejected `{method}`: {message}")]
    Rpc { method: String, message: String },
    #[error("Codex app-server response for `{method}` omitted `{field}`")]
    MissingField { method: String, field: &'static str },
    #[error("Codex turn `{turn_id}` failed with status `{status}`")]
    TurnFailed { turn_id: String, status: String },
    #[error("Codex turn `{turn_id}` was interrupted")]
    Cancelled { turn_id: String },
    #[error("Codex turn `{turn_id}` completed without structured output")]
    MissingOutput { turn_id: String },
}

pub fn codex_app_server_capability(
    config: &CodexAppServerConfig,
) -> Result<PathBuf, CodexAppServerError> {
    if !config.enabled {
        return Err(CodexAppServerError::Disabled);
    }
    resolve_executable(&config.executable).ok_or_else(|| {
        CodexAppServerError::ExecutableUnavailable(config.executable.display().to_string())
    })
}

fn resolve_executable(executable: &Path) -> Option<PathBuf> {
    if executable.components().count() > 1 {
        return is_executable_file(executable).then(|| executable.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(executable))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub struct CodexAppServerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    queued_notifications: VecDeque<Value>,
}

impl CodexAppServerClient {
    pub async fn spawn(config: &CodexAppServerConfig) -> Result<Self, CodexAppServerError> {
        let executable = codex_app_server_capability(config)?;
        let mut child = Command::new(executable)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(CodexAppServerError::Spawn)?;
        let stdin = child.stdin.take().ok_or(CodexAppServerError::MissingPipe)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(CodexAppServerError::MissingPipe)?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            queued_notifications: VecDeque::new(),
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<(), CodexAppServerError> {
        let result = self.request(
            "initialize",
            json!({
                "clientInfo": {"name": "bandicot-agentgraph", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }),
        )
        .await?;
        if !result.is_object() {
            return Err(CodexAppServerError::MissingField {
                method: "initialize".to_string(),
                field: "result object",
            });
        }
        self.notify("initialized", json!({})).await
    }

    pub async fn start_thread(
        &mut self,
        options: &CodexWorkerOptions,
    ) -> Result<String, CodexAppServerError> {
        let result = self
            .request(
                "thread/start",
                json!({
                    "cwd": options.cwd,
                    "model": options.model,
                    "approvalPolicy": "never",
                    "sandbox": "read-only"
                }),
            )
            .await?;
        thread_id(&result, "thread/start")
    }

    pub async fn resume_thread(&mut self, thread_id: &str) -> Result<String, CodexAppServerError> {
        let result = self
            .request("thread/resume", json!({"threadId": thread_id}))
            .await?;
        thread_id_from_resume(&result, thread_id, "thread/resume")
    }

    pub async fn fork_thread(
        &mut self,
        source_thread_id: &str,
    ) -> Result<String, CodexAppServerError> {
        let result = self
            .request("thread/fork", json!({"threadId": source_thread_id}))
            .await?;
        thread_id(&result, "thread/fork")
    }

    pub async fn start_turn(
        &mut self,
        thread_id: &str,
        prompt: &str,
        options: &CodexWorkerOptions,
    ) -> Result<String, CodexAppServerError> {
        let result = self
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "input": [{"type": "text", "text": prompt}],
                    "model": options.model,
                    "effort": options.reasoning_effort,
                    "outputSchema": options.output_schema
                }),
            )
            .await?;
        value_string(&result, &["turn", "id"])
            .or_else(|| value_string(&result, &["turnId"]))
            .ok_or(CodexAppServerError::MissingField {
                method: "turn/start".to_string(),
                field: "turn.id",
            })
    }

    pub async fn interrupt(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), CodexAppServerError> {
        self.request(
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": turn_id}),
        )
        .await?;
        Ok(())
    }

    pub async fn collect_turn(
        &mut self,
        thread_id: String,
        turn_id: String,
    ) -> Result<CodexTurnResult, CodexAppServerError> {
        self.collect_turn_inner(thread_id, turn_id, None).await
    }

    async fn collect_turn_with_cancel(
        &mut self,
        thread_id: String,
        turn_id: String,
        cancel: &mut oneshot::Receiver<()>,
    ) -> Result<CodexTurnResult, CodexAppServerError> {
        self.collect_turn_inner(thread_id, turn_id, Some(cancel))
            .await
    }

    async fn collect_turn_inner(
        &mut self,
        thread_id: String,
        turn_id: String,
        mut cancel: Option<&mut oneshot::Receiver<()>>,
    ) -> Result<CodexTurnResult, CodexAppServerError> {
        let mut notifications = Vec::new();
        let mut output = None;
        loop {
            let message = if let Some(message) = self.queued_notifications.pop_front() {
                message
            } else if cancel.is_some() {
                enum NextMessage {
                    Message(Result<Value, CodexAppServerError>),
                    Cancel,
                }
                let next = {
                    let cancel_receiver = cancel.as_deref_mut().expect("cancel receiver exists");
                    tokio::select! {
                        message = self.read_message() => NextMessage::Message(message),
                        _ = cancel_receiver => NextMessage::Cancel,
                    }
                };
                match next {
                    NextMessage::Message(message) => message?,
                    NextMessage::Cancel => {
                        self.interrupt(&thread_id, &turn_id).await?;
                        return Err(CodexAppServerError::Cancelled { turn_id });
                    }
                }
            } else {
                self.read_message().await?
            };
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            let belongs_to_turn =
                notification_turn_id(&message).is_none_or(|id| id == turn_id.as_str());
            if method == "item/completed" && belongs_to_turn {
                if let Some(candidate) = completed_item_output(&message) {
                    output = Some(candidate);
                }
            }
            let is_turn_complete = method == "turn/completed" && belongs_to_turn;
            notifications.push(message.clone());
            if !is_turn_complete {
                continue;
            }
            let status = message
                .pointer("/params/turn/status")
                .or_else(|| message.pointer("/params/status"))
                .and_then(Value::as_str)
                .unwrap_or("completed");
            if !matches!(status, "completed" | "succeeded") {
                return Err(CodexAppServerError::TurnFailed {
                    turn_id,
                    status: status.to_string(),
                });
            }
            let output = output
                .or_else(|| message.pointer("/params/output").cloned())
                .ok_or_else(|| CodexAppServerError::MissingOutput {
                    turn_id: turn_id.clone(),
                })?;
            return Ok(CodexTurnResult {
                thread_id,
                turn_id,
                output,
                notifications,
            });
        }
    }

    pub async fn shutdown(mut self) -> Result<(), CodexAppServerError> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        let _ = self.child.wait().await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, CodexAppServerError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_message(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .await?;
        loop {
            let message = self.read_message().await?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                if message.get("method").is_some() {
                    self.queued_notifications.push_back(message);
                }
                continue;
            }
            if let Some(error) = message.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown JSON-RPC error");
                return Err(CodexAppServerError::Rpc {
                    method: method.to_string(),
                    message: message.to_string(),
                });
            }
            return message
                .get("result")
                .cloned()
                .ok_or(CodexAppServerError::MissingField {
                    method: method.to_string(),
                    field: "result",
                });
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), CodexAppServerError> {
        self.write_message(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn write_message(&mut self, message: &Value) -> Result<(), CodexAppServerError> {
        let mut encoded = serde_json::to_vec(message)?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Value, CodexAppServerError> {
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).await? == 0 {
                return Err(CodexAppServerError::UnexpectedEof);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return serde_json::from_str(trimmed).map_err(CodexAppServerError::InvalidJson);
        }
    }
}

impl Drop for CodexAppServerClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl WorkerBackend for CodexAppServerWorkerBackend {
    fn start(&mut self, request: WorkerRequest) -> Result<WorkerHandle, WorkerError> {
        codex_app_server_capability(&self.config).map_err(|error| WorkerError::Rejected {
            node_id: request.node_id.clone(),
            reason: error.to_string(),
        })?;
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|error| WorkerError::Rejected {
                node_id: request.node_id.clone(),
                reason: format!("Codex app-server worker requires a Tokio runtime: {error}"),
            })?;
        let worker_id = graph_worker_id(&request.graph_run_id, &request.node_id, request.attempt);
        if self.active.contains_key(&worker_id) {
            return Err(WorkerError::Rejected {
                node_id: request.node_id.clone(),
                reason: format!("worker `{worker_id}` is already active"),
            });
        }

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();
        let config = self.config.clone();
        let options = self.options.clone();
        let task_request = request.clone();
        runtime.spawn(async move {
            let completion = run_worker(config, options, task_request, cancel_rx).await;
            let _ = result_tx.send(completion);
        });
        self.active.insert(
            worker_id.clone(),
            ActiveCodexWorker {
                request: request.clone(),
                cancel: Some(cancel_tx),
                result: result_rx,
            },
        );
        Ok(WorkerHandle {
            worker_id,
            node_id: request.node_id,
            attempt: request.attempt,
        })
    }

    fn poll_completed(&mut self) -> Vec<WorkerCompletion> {
        let mut completed = Vec::new();
        let worker_ids = self.active.keys().cloned().collect::<Vec<_>>();
        for worker_id in worker_ids {
            let result = self
                .active
                .get_mut(&worker_id)
                .map(|active| active.result.try_recv());
            match result {
                Some(Ok(completion)) => {
                    self.active.remove(&worker_id);
                    completed.push(completion);
                }
                Some(Err(oneshot::error::TryRecvError::Closed)) => {
                    if let Some(active) = self.active.remove(&worker_id) {
                        completed.push(failed_completion(
                            &active.request,
                            NodeStatus::Failed,
                            "Codex app-server worker exited without a result",
                        ));
                    }
                }
                Some(Err(oneshot::error::TryRecvError::Empty)) | None => {}
            }
        }
        completed
    }

    fn cancel(&mut self, worker_id: &str) -> Result<(), WorkerError> {
        let active = self
            .active
            .get_mut(worker_id)
            .ok_or_else(|| WorkerError::NotFound {
                worker_id: worker_id.to_string(),
            })?;
        if let Some(cancel) = active.cancel.take() {
            let _ = cancel.send(());
        }
        Ok(())
    }

    fn active_count(&self) -> usize {
        self.active.len()
    }
}

async fn run_worker(
    config: CodexAppServerConfig,
    options: CodexWorkerOptions,
    request: WorkerRequest,
    mut cancel: oneshot::Receiver<()>,
) -> WorkerCompletion {
    let mut client = match CodexAppServerClient::spawn(&config).await {
        Ok(client) => client,
        Err(error) => {
            return failed_completion(&request, NodeStatus::Failed, &error.to_string());
        }
    };
    let thread_id = match client.start_thread(&options).await {
        Ok(thread_id) => thread_id,
        Err(error) => {
            let _ = client.shutdown().await;
            return failed_completion(&request, NodeStatus::Failed, &error.to_string());
        }
    };
    let turn_id = match client
        .start_turn(&thread_id, &request.objective, &options)
        .await
    {
        Ok(turn_id) => turn_id,
        Err(error) => {
            let _ = client.shutdown().await;
            return failed_completion(&request, NodeStatus::Failed, &error.to_string());
        }
    };

    let result = client
        .collect_turn_with_cancel(thread_id, turn_id, &mut cancel)
        .await;
    let _ = client.shutdown().await;
    match result {
        Ok(result) => match serde_json::from_value::<NodeOutput>(result.output) {
            Ok(output) => completion_from_output(&request, output),
            Err(error) => failed_completion(
                &request,
                NodeStatus::Failed,
                &format!("Codex app-server output was not valid NodeOutput JSON: {error}"),
            ),
        },
        Err(CodexAppServerError::Cancelled { .. }) => failed_completion(
            &request,
            NodeStatus::Cancelled,
            "Codex app-server worker was cancelled",
        ),
        Err(CodexAppServerError::TurnFailed { status, .. }) if status == "interrupted" => {
            failed_completion(
                &request,
                NodeStatus::Cancelled,
                "Codex app-server worker was cancelled",
            )
        }
        Err(error) => failed_completion(&request, NodeStatus::Failed, &error.to_string()),
    }
}

fn completion_from_output(request: &WorkerRequest, output: NodeOutput) -> WorkerCompletion {
    let expected_attempt = format!("attempt-{}", request.attempt);
    let mismatch = if output.schema_version != AGENTGRAPH_SCHEMA_VERSION {
        Some(format!(
            "Codex app-server output schema version {} did not match {}",
            output.schema_version, AGENTGRAPH_SCHEMA_VERSION
        ))
    } else if output.graph_run_id != request.graph_run_id {
        Some("Codex app-server output graphRunId did not match the worker request".to_string())
    } else if output.node_instance_id != request.node_id {
        Some("Codex app-server output nodeInstanceId did not match the worker request".to_string())
    } else if output.attempt_id != expected_attempt {
        Some("Codex app-server output attemptId did not match the worker request".to_string())
    } else if !output.status.is_terminal() {
        Some("Codex app-server output did not contain a terminal node status".to_string())
    } else {
        None
    };
    if let Some(reason) = mismatch {
        return failed_completion(request, NodeStatus::Failed, &reason);
    }
    WorkerCompletion {
        graph_run_id: request.graph_run_id.clone(),
        node_id: request.node_id.clone(),
        attempt: request.attempt,
        output,
        usage: UsageAccounting {
            model_calls: 1,
            node_attempts: 1,
            ..UsageAccounting::default()
        },
    }
}

fn failed_completion(
    request: &WorkerRequest,
    status: NodeStatus,
    summary: &str,
) -> WorkerCompletion {
    WorkerCompletion {
        graph_run_id: request.graph_run_id.clone(),
        node_id: request.node_id.clone(),
        attempt: request.attempt,
        output: NodeOutput {
            schema_version: AGENTGRAPH_SCHEMA_VERSION,
            graph_run_id: request.graph_run_id.clone(),
            node_instance_id: request.node_id.clone(),
            attempt_id: format!("attempt-{}", request.attempt),
            status,
            summary: summary.to_string(),
            findings: Vec::new(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            tests_run: Vec::new(),
            artifacts: Vec::new(),
            assumptions: Vec::new(),
            blockers: vec![summary.to_string()],
            confidence: 0.0,
        },
        usage: UsageAccounting {
            model_calls: 1,
            node_attempts: 1,
            failures: u64::from(status != NodeStatus::Succeeded),
            ..UsageAccounting::default()
        },
    }
}

fn thread_id(result: &Value, method: &str) -> Result<String, CodexAppServerError> {
    value_string(result, &["thread", "id"])
        .or_else(|| value_string(result, &["threadId"]))
        .ok_or(CodexAppServerError::MissingField {
            method: method.to_string(),
            field: "thread.id",
        })
}

fn thread_id_from_resume(
    result: &Value,
    requested: &str,
    method: &str,
) -> Result<String, CodexAppServerError> {
    if result.is_null() {
        Ok(requested.to_string())
    } else {
        thread_id(result, method)
    }
}

fn value_string(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))?
        .as_str()
        .map(ToString::to_string)
}

fn notification_turn_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/turn/id")
        .or_else(|| message.pointer("/params/turnId"))
        .and_then(Value::as_str)
}

fn completed_item_output(message: &Value) -> Option<Value> {
    let item = message.pointer("/params/item")?;
    if let Some(output) = item.get("structuredOutput") {
        return Some(output.clone());
    }
    if let Some(text) = item.get("text").and_then(Value::as_str) {
        return serde_json::from_str(text)
            .ok()
            .or_else(|| Some(Value::String(text.to_string())));
    }
    item.get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|part| part.get("text").and_then(Value::as_str))
        })
        .map(|text| serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_is_disabled_and_read_only_by_default() {
        let config = CodexAppServerConfig::default();
        assert!(!config.enabled);
        assert!(matches!(
            codex_app_server_capability(&config),
            Err(CodexAppServerError::Disabled)
        ));
        let options = CodexWorkerOptions {
            cwd: PathBuf::from("/tmp"),
            model: None,
            reasoning_effort: None,
            output_schema: None,
        };
        let params = json!({"cwd": options.cwd, "approvalPolicy": "never", "sandbox": "read-only"});
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["approvalPolicy"], "never");
    }

    #[test]
    fn parses_structured_and_text_item_outputs() {
        let structured = json!({"params":{"item":{"structuredOutput":{"ok":true}}}});
        assert_eq!(completed_item_output(&structured), Some(json!({"ok":true})));
        let text = json!({"params":{"item":{"text":"{\"count\":3}"}}});
        assert_eq!(completed_item_output(&text), Some(json!({"count":3})));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_json_rpc_server_exercises_lifecycle_without_codex() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake-codex-app-server.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"fake"}}}' ;;
    *'"method":"initialized"'*) ;;
    *'"method":"thread/start"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"thread-1"}}}' ;;
    *'"method":"thread/resume"'*) echo '{"jsonrpc":"2.0","id":3,"result":{"thread":{"id":"thread-1"}}}' ;;
    *'"method":"thread/fork"'*) echo '{"jsonrpc":"2.0","id":4,"result":{"thread":{"id":"thread-2"}}}' ;;
    *'"method":"turn/start"'*)
      echo '{"jsonrpc":"2.0","id":5,"result":{"turn":{"id":"turn-1"}}}'
      echo '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"structuredOutput":{"answer":42}}}}'
      echo '{"jsonrpc":"2.0","method":"turn/completed","params":{"turn":{"id":"turn-1","status":"completed"}}}'
      ;;
    *'"method":"turn/interrupt"'*) echo '{"jsonrpc":"2.0","id":6,"result":{}}' ;;
  esac
done
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let config = CodexAppServerConfig {
            enabled: true,
            executable: script,
            args: vec![],
        };
        let options = CodexWorkerOptions {
            cwd: temp.path().to_path_buf(),
            model: Some("gpt-5.6-sol".to_string()),
            reasoning_effort: Some("medium".to_string()),
            output_schema: Some(json!({"type":"object"})),
        };
        let mut client = CodexAppServerClient::spawn(&config).await.unwrap();
        let thread_id = client.start_thread(&options).await.unwrap();
        assert_eq!(client.resume_thread(&thread_id).await.unwrap(), "thread-1");
        assert_eq!(client.fork_thread(&thread_id).await.unwrap(), "thread-2");
        let turn_id = client
            .start_turn(&thread_id, "Return JSON", &options)
            .await
            .unwrap();
        client.interrupt(&thread_id, &turn_id).await.unwrap();
        let result = client.collect_turn(thread_id, turn_id).await.unwrap();
        assert_eq!(result.output, json!({"answer":42}));
        assert_eq!(result.notifications.len(), 2);
        client.shutdown().await.unwrap();
    }
}
