use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::approval::{
    ApprovalBinding, ExecutionApproval, graph_requires_execution_approval,
    resolve_repository_commit,
};
use super::benchmark::{build_exact_100_worker_graph, run_exact_100_fake_benchmark};
use super::scheduler::{AgentGraphScheduler, SchedulerConfig};
use super::store::AgentGraphStore;
use super::types::{GraphSpec, RunStatus};
use super::validation::{ValidationOptions, validate_graph_spec};
use super::worker::SubagentWorkerBackend;
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend as ExistingSubagentBackend;

pub const ULTRA_MAX_CHILDREN: u32 = 6;
pub const ULTRA_DEPTH_LIMIT: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UltraSettingSource {
    #[default]
    Default,
    Config,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UltraOrchestrationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ultra_max_children")]
    pub max_children: u32,
    /// Whether the policy matching `enabled` has already been injected.
    ///
    /// The default off state is already effective without an injected block.
    /// Explicit transitions set this false, causing exactly one tagged policy
    /// update to be added at the start of the next user turn.
    #[serde(default = "default_ultra_policy_injected")]
    pub policy_injected: bool,
    #[serde(default)]
    pub setting_source: UltraSettingSource,
}

impl Default for UltraOrchestrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_children: ULTRA_MAX_CHILDREN,
            policy_injected: true,
            setting_source: UltraSettingSource::Default,
        }
    }
}

impl UltraOrchestrationConfig {
    pub fn normalized(mut self) -> Self {
        self.max_children = clamp_ultra_children(self.max_children);
        self
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_ultra_max_children() -> u32 {
    ULTRA_MAX_CHILDREN
}

fn default_ultra_policy_injected() -> bool {
    true
}

pub fn clamp_ultra_children(value: u32) -> u32 {
    value.clamp(1, ULTRA_MAX_CHILDREN)
}

pub fn ultra_has_child_capacity(limit: u32, active: usize, pending: usize) -> bool {
    active.saturating_add(pending) < clamp_ultra_children(limit) as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UltraCommand {
    On { max_children: u32 },
    Off,
    Status,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastCommand {
    On,
    Off,
    Status,
    Help,
}

pub fn parse_fast_command(args: &str) -> FastCommand {
    match args.trim().to_ascii_lowercase().as_str() {
        "on" | "enable" => FastCommand::On,
        "off" | "disable" => FastCommand::Off,
        "" | "status" => FastCommand::Status,
        _ => FastCommand::Help,
    }
}

pub fn parse_ultra_command(args: &str) -> UltraCommand {
    let mut parts = args.split_whitespace();
    match parts
        .next()
        .unwrap_or("status")
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "status" => UltraCommand::Status,
        "off" | "disable" => UltraCommand::Off,
        "on" | "enable" => {
            let mut max_children = ULTRA_MAX_CHILDREN;
            while let Some(part) = parts.next() {
                match part {
                    "--max-children" | "--limit" => {
                        let Some(value) = parts.next().and_then(|raw| raw.parse::<u32>().ok())
                        else {
                            return UltraCommand::Help;
                        };
                        max_children = clamp_ultra_children(value);
                    }
                    raw => {
                        let Ok(value) = raw.parse::<u32>() else {
                            return UltraCommand::Help;
                        };
                        max_children = clamp_ultra_children(value);
                    }
                }
            }
            UltraCommand::On { max_children }
        }
        _ => UltraCommand::Help,
    }
}

pub fn format_ultra_status(config: &UltraOrchestrationConfig, is_subagent: bool) -> String {
    let state = ultra_config_for_session(*config, is_subagent);
    if is_subagent {
        return format!(
            "Ultra orchestration\nRequested state: off\nEffective state: off (internal child session)\nSetting source: enforced child policy\nMaximum children: 0\nDepth limit: {ULTRA_DEPTH_LIMIT}\nChild sessions cannot enable Ultra or spawn nested orchestration."
        );
    }
    let requested = if state.enabled { "on" } else { "off" };
    let effective = if state.enabled { "on" } else { "off" };
    let source = match state.setting_source {
        UltraSettingSource::Default => "default",
        UltraSettingSource::Config => "config",
        UltraSettingSource::Session => "session",
    };
    format!(
        "Ultra orchestration\nRequested state: {requested}\nEffective state: {effective}\nSetting source: {source}\nMaximum children: {}\nDepth limit: {ULTRA_DEPTH_LIMIT}\nPolicy applied: {}",
        state.max_children,
        if state.policy_injected {
            "yes"
        } else {
            "pending next user turn"
        }
    )
}

pub fn ultra_config_for_session(
    config: UltraOrchestrationConfig,
    is_subagent: bool,
) -> UltraOrchestrationConfig {
    if is_subagent {
        UltraOrchestrationConfig::default()
    } else {
        config.normalized()
    }
}

pub fn ultra_policy_text(max_children: u32) -> String {
    format!(
        "<bandicot-orchestration mode=\"ultra\">\n\
         Ultra orchestration is enabled for this user-facing root session.\n\
         You may proactively delegate independent read, research, test, or verification work to at most {} child agents when that improves progress.\n\
         Prefer two to four children; six is a ceiling, not a target. Keep delegation depth at one.\n\
         Keep all child agents non-recursive: they must not spawn agents, use `/graph`, `/swarm`, `/ultra`, or provider-hosted multi-agent features.\n\
         Do not change the selected model, reasoning effort, service tier, permissions, sandbox, plan mode, or approval policy because Ultra is on.\n\
         In Plan Mode, dispatch only read-only or planning workers.\n\
         Coordinate child work from the root session and verify outputs locally before treating them as complete.\n\
         </bandicot-orchestration>",
        clamp_ultra_children(max_children)
    )
}

pub fn ultra_off_policy_text() -> String {
    "<bandicot-orchestration mode=\"standard\">\n\
     Ultra proactive delegation is disabled. Preserve existing explicit subagent features, but do not proactively delegate because of a prior Ultra policy.\n\
     Already running children may finish; do not cancel them because Ultra was disabled.\n\
     </bandicot-orchestration>"
        .to_string()
}

pub fn take_ultra_policy_for_root(
    config: &mut UltraOrchestrationConfig,
    is_subagent: bool,
) -> Option<String> {
    if is_subagent {
        *config = UltraOrchestrationConfig::default();
        return None;
    }
    *config = config.normalized();
    if config.policy_injected {
        return None;
    }
    config.policy_injected = true;
    Some(if config.enabled {
        ultra_policy_text(config.max_children)
    } else {
        ultra_off_policy_text()
    })
}

#[cfg(test)]
mod ultra_tests {
    use super::*;

    #[test]
    fn default_off_state_does_not_inject_policy() {
        let mut config = UltraOrchestrationConfig::default();
        assert!(!config.enabled);
        assert!(take_ultra_policy_for_root(&mut config, false).is_none());
    }

    #[test]
    fn enabled_policy_is_root_only_tagged_and_injected_once() {
        let mut config = UltraOrchestrationConfig {
            enabled: true,
            max_children: ULTRA_MAX_CHILDREN,
            policy_injected: false,
            setting_source: UltraSettingSource::Session,
        };
        let policy = take_ultra_policy_for_root(&mut config, false).expect("first injection");
        assert!(policy.contains("<bandicot-orchestration mode=\"ultra\">"));
        assert!(policy.contains("at most 6 child agents"));
        assert!(policy.contains("depth at one"));
        assert!(policy.contains("must not spawn agents"));
        assert!(take_ultra_policy_for_root(&mut config, false).is_none());

        let mut child = UltraOrchestrationConfig {
            enabled: true,
            max_children: 6,
            policy_injected: false,
            setting_source: UltraSettingSource::Session,
        };
        assert!(take_ultra_policy_for_root(&mut child, true).is_none());
        assert_eq!(child, UltraOrchestrationConfig::default());
    }

    #[test]
    fn disabling_injects_non_proactive_policy_once_without_cancel_directive() {
        let mut config = UltraOrchestrationConfig {
            enabled: false,
            max_children: ULTRA_MAX_CHILDREN,
            policy_injected: false,
            setting_source: UltraSettingSource::Session,
        };
        let policy = take_ultra_policy_for_root(&mut config, false).expect("off transition");
        assert!(policy.contains("mode=\"standard\""));
        assert!(policy.contains("Already running children may finish"));
        assert!(!policy.contains("cancel all"));
        assert!(take_ultra_policy_for_root(&mut config, false).is_none());
    }

    #[test]
    fn capacity_is_clamped_to_explicit_six_child_ceiling() {
        assert_eq!(clamp_ultra_children(0), 1);
        assert_eq!(clamp_ultra_children(4), 4);
        assert_eq!(clamp_ultra_children(99), ULTRA_MAX_CHILDREN);
        assert!(ultra_has_child_capacity(6, 4, 1));
        assert!(!ultra_has_child_capacity(6, 5, 1));
        assert!(!ultra_has_child_capacity(6, 6, 0));
    }

    #[test]
    fn requested_state_and_source_round_trip_for_resume_and_fork() {
        let requested = UltraOrchestrationConfig {
            enabled: true,
            max_children: 4,
            policy_injected: false,
            setting_source: UltraSettingSource::Session,
        };
        let encoded = serde_json::to_vec(&requested).expect("serialize Ultra state");
        let restored: UltraOrchestrationConfig =
            serde_json::from_slice(&encoded).expect("restore Ultra state");
        assert_eq!(restored, requested);
        assert_eq!(ultra_config_for_session(restored, false), requested);
        assert_eq!(
            ultra_config_for_session(restored, true),
            UltraOrchestrationConfig::default()
        );
    }

    #[test]
    fn policy_does_not_request_changes_to_independent_runtime_axes() {
        let policy = ultra_policy_text(4);
        for invariant in [
            "selected model",
            "reasoning effort",
            "service tier",
            "permissions",
            "sandbox",
            "plan mode",
            "approval policy",
        ] {
            assert!(policy.contains(invariant));
        }
        assert!(!policy.contains("YOLO on"));
        assert!(!policy.contains("Fast on"));
    }
}

#[derive(Clone)]
pub struct AgentGraphControlPlane {
    cwd: PathBuf,
    store_path: PathBuf,
    session_id: Option<String>,
    models_manager: Option<crate::agent::models::ModelsManager>,
}

impl AgentGraphControlPlane {
    pub fn new(cwd: &Path, session_id: Option<&str>) -> Self {
        let agent_dir = cwd.join(".agent");
        Self {
            cwd: cwd.to_path_buf(),
            store_path: agent_dir.join("agentgraph.db"),
            session_id: session_id.map(ToString::to_string),
            models_manager: None,
        }
    }

    pub fn with_models_manager(
        mut self,
        models_manager: crate::agent::models::ModelsManager,
    ) -> Self {
        self.models_manager = Some(models_manager);
        self
    }

    pub fn status(&self, label: &str) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return format!("{label}: no active run is attached to this session."),
            Err(err) => return err,
        };
        match AgentGraphStore::open(&self.store_path).and_then(|store| store.run_status(&run_id)) {
            Ok(status) => format!("{label}: active run {run_id}\nStatus: {status:?}"),
            Err(err) => format!("{label}: active run {run_id} could not be loaded: {err}"),
        }
    }

    pub fn create_run_from_spec(&self, spec: &GraphSpec) -> String {
        if let Err(err) = validate_graph_for_command(spec) {
            return err;
        }
        match self.create_run(spec) {
            Ok((run_id, session_id)) => format!(
                "Graph run created and awaiting approval.\nRun: {run_id}\nSession: {session_id}\nExecution: not started."
            ),
            Err(err) => err,
        }
    }

    pub fn run_spec_now(&self, spec: GraphSpec) -> String {
        if let Err(err) = validate_graph_for_command(&spec) {
            return err;
        }
        match self.create_run(&spec) {
            Ok((run_id, _session_id)) => self.execution_unavailable_message(&run_id),
            Err(err) => err,
        }
    }

    pub fn run_spec_now_with_backend(
        &self,
        spec: GraphSpec,
        backend: Arc<dyn ExistingSubagentBackend>,
    ) -> String {
        if let Err(err) = validate_graph_for_command(&spec) {
            return err;
        }
        let (run_id, _session_id) = match self.create_run(&spec) {
            Ok(created) => created,
            Err(err) => return err,
        };
        if let Err(message) = self.require_execution_approval(&run_id, &spec) {
            return message;
        }
        self.spawn_stored_spec_with_backend(run_id, spec, backend)
    }

    pub fn run_fake_spec_now(&self, spec: GraphSpec) -> String {
        if let Err(err) = validate_graph_for_command(&spec) {
            return err;
        }
        let (run_id, _session_id) = match self.create_run(&spec) {
            Ok(created) => created,
            Err(err) => return err,
        };
        let store = match AgentGraphStore::open(&self.store_path) {
            Ok(store) => store,
            Err(err) => return format!("Failed to open AgentGraph store: {err}"),
        };
        self.run_stored_spec_fake(store, run_id, spec)
    }

    pub fn run_active(&self) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return "No active graph run. Use `/graph plan <path>` first.".to_string(),
            Err(err) => return err,
        };
        self.execution_unavailable_message(&run_id)
    }

    pub fn run_active_with_backend(&self, backend: Arc<dyn ExistingSubagentBackend>) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return "No active graph run. Use `/graph plan <path>` first.".to_string(),
            Err(err) => return err,
        };
        let store = match AgentGraphStore::open(&self.store_path) {
            Ok(store) => store,
            Err(err) => return format!("Failed to open AgentGraph store: {err}"),
        };
        let status = match store.run_status(&run_id) {
            Ok(status) => status,
            Err(err) => return format!("Failed to load graph run {run_id}: {err}"),
        };
        if status == RunStatus::Running {
            return format!(
                "Graph run {run_id} is already running.\nUse `/graph status` to monitor it."
            );
        }
        if matches!(
            status,
            RunStatus::Completed
                | RunStatus::Failed
                | RunStatus::Cancelled
                | RunStatus::BudgetStopped
                | RunStatus::PartiallyCompleted
        ) {
            return format!("Graph run {run_id} is terminal ({status:?}) and was not restarted.");
        }
        let spec = match store.graph_spec_for_run(&run_id) {
            Ok(spec) => spec,
            Err(err) => return format!("Failed to load graph run {run_id}: {err}"),
        };
        if let Err(message) = self.require_execution_approval(&run_id, &spec) {
            return message;
        }
        drop(store);
        self.spawn_stored_spec_with_backend(run_id, spec, backend)
    }

    pub fn approval_material(&self) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return "No active graph run to approve.".to_string(),
            Err(err) => return err,
        };
        let store = match AgentGraphStore::open(&self.store_path) {
            Ok(store) => store,
            Err(err) => return format!("Failed to open AgentGraph store: {err}"),
        };
        let spec = match store.graph_spec_for_run(&run_id) {
            Ok(spec) => spec,
            Err(err) => return format!("Failed to load graph run {run_id}: {err}"),
        };
        let commit = match resolve_repository_commit(&self.cwd) {
            Ok(commit) => commit,
            Err(err) => return format!("Approval preflight failed: {err}"),
        };
        let expires_at_ms = super::store::now_ms().saturating_add(15 * 60 * 1000);
        match ApprovalBinding::for_spec(&spec, commit, expires_at_ms) {
            Ok(binding) => serde_json::to_string_pretty(&binding)
                .unwrap_or_else(|err| format!("Failed to serialize approval material: {err}")),
            Err(err) => format!("Approval preflight failed: {err}"),
        }
    }

    pub fn approve_active(&self, expected_graph_hash: &str) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return "No active graph run to approve.".to_string(),
            Err(err) => return err,
        };
        let mut store = match AgentGraphStore::open(&self.store_path) {
            Ok(store) => store,
            Err(err) => return format!("Failed to open AgentGraph store: {err}"),
        };
        let spec = match store.graph_spec_for_run(&run_id) {
            Ok(spec) => spec,
            Err(err) => return format!("Failed to load graph run {run_id}: {err}"),
        };
        let commit = match resolve_repository_commit(&self.cwd) {
            Ok(commit) => commit,
            Err(err) => return format!("Approval preflight failed: {err}"),
        };
        let now = super::store::now_ms();
        let binding = match ApprovalBinding::for_spec(&spec, commit, now + 15 * 60 * 1000) {
            Ok(binding) => binding,
            Err(err) => return format!("Approval preflight failed: {err}"),
        };
        if binding.normalized_graph_hash != expected_graph_hash {
            return format!(
                "Approval refused: graph hash mismatch. Expected acknowledgment: {}",
                binding.normalized_graph_hash
            );
        }
        let approval = ExecutionApproval {
            binding,
            approved_at_ms: now,
            acknowledgment: "explicit graph-hash acknowledgment".to_string(),
        };
        match store.save_execution_approval(&run_id, &approval) {
            Ok(()) => format!("Graph run {run_id} approved for 15 minutes."),
            Err(err) => format!("Failed to persist approval for {run_id}: {err}"),
        }
    }

    pub fn transition_active(&self, status: RunStatus, verb: &str) -> String {
        let run_id = match self.active_run_id() {
            Ok(Some(run_id)) => run_id,
            Ok(None) => return format!("No active graph run to {verb}."),
            Err(err) => return err,
        };
        let mut store = match AgentGraphStore::open(&self.store_path) {
            Ok(store) => store,
            Err(err) => return format!("Failed to open AgentGraph store: {err}"),
        };
        match store.mark_run_status(&run_id, status) {
            Ok(()) => {
                format!("Graph run {run_id}: control request recorded: {verb} -> {status:?}")
            }
            Err(err) => format!("Failed to {verb} graph run {run_id}: {err}"),
        }
    }

    fn create_run(&self, spec: &GraphSpec) -> Result<(String, String), String> {
        let session_id = self.require_session_id()?.to_string();
        let mut store = AgentGraphStore::open(&self.store_path)
            .map_err(|err| format!("Failed to open AgentGraph store: {err}"))?;
        let run_id = store
            .create_run(spec, Some(&session_id), &self.cwd)
            .map_err(|err| format!("Failed to create graph run: {err}"))?;
        store
            .attach_active_run(&session_id, &self.cwd, &run_id)
            .map_err(|err| format!("Failed to attach active graph run: {err}"))?;
        Ok((run_id, session_id))
    }

    fn require_execution_approval(&self, run_id: &str, spec: &GraphSpec) -> Result<(), String> {
        if spec.spec.execution.orchestration_policy == super::types::OrchestrationMode::Swarm
            && (!spec.spec.budgets.hard_stop
                || (spec.spec.budgets.max_input_tokens.is_none()
                    && spec.spec.budgets.max_output_tokens.is_none()
                    && spec.spec.budgets.max_model_calls.is_none()))
        {
            return Err(
                "Live Swarm preflight failed: a hard token or model-call budget is required."
                    .to_string(),
            );
        }
        if !graph_requires_execution_approval(spec) {
            return Ok(());
        }
        let store = AgentGraphStore::open(&self.store_path)
            .map_err(|err| format!("Failed to open AgentGraph store: {err}"))?;
        let approval = store
            .execution_approval(run_id)
            .map_err(|err| format!("Failed to load execution approval: {err}"))?
            .ok_or_else(|| {
                format!(
                    "Graph run {run_id} requires an approval bound to its immutable graph, budget, effects, permissions, repository commit, and expiry.\nUse `/graph approval` to inspect the material, then `/graph approve <normalized-graph-hash>`."
                )
            })?;
        let commit = resolve_repository_commit(&self.cwd)
            .map_err(|err| format!("Approval verification failed: {err}"))?;
        approval
            .binding
            .verify(spec, &commit, super::store::now_ms())
            .map_err(|err| format!("Execution approval is no longer valid: {err}"))
    }

    fn execution_unavailable_message(&self, run_id: &str) -> String {
        let status = AgentGraphStore::open(&self.store_path)
            .and_then(|store| store.run_status(run_id))
            .map(|status| format!("{status:?}"))
            .unwrap_or_else(|err| format!("<unavailable: {err}>"));
        format!(
            "Graph run {run_id} was not executed.\nStatus: {status}\nExecution backend: unavailable for this command surface. No fake worker results were recorded."
        )
    }

    fn run_stored_spec_fake(
        &self,
        mut store: AgentGraphStore,
        run_id: String,
        spec: GraphSpec,
    ) -> String {
        if let Err(err) = store.mark_run_status(&run_id, RunStatus::Running) {
            return format!("Failed to mark graph run {run_id} running: {err}");
        }
        let config = SchedulerConfig::from_graph(&spec);
        let mut scheduler = match AgentGraphScheduler::new(spec, config) {
            Ok(scheduler) => scheduler,
            Err(err) => return format!("Failed to build graph scheduler: {err}"),
        };
        let mut backend = super::worker::FakeWorkerBackend::complete_immediately();
        let report = match scheduler.run_to_completion(&mut backend, &run_id) {
            Ok(report) => report,
            Err(err) => {
                let _ = store.mark_run_status(&run_id, RunStatus::Failed);
                return format!("Fake graph run {run_id} failed: {err}");
            }
        };
        let final_status = report.run_status;
        let _ = store.mark_run_status(&run_id, final_status);
        format!(
            "Fake graph run {run_id} finished.\nStatus: {final_status:?}\nNodes: {}\nCompleted: {}\nFailed: {}\nTimed out: {}\nCancelled: {}\nPeak active: {}\nQueued: {}\nBackend: fake",
            report.total_nodes,
            report.completed_nodes,
            report.failed_nodes,
            report.timed_out_nodes,
            report.cancelled_nodes,
            report.peak_active_workers,
            report.queued_workers
        )
    }

    fn spawn_stored_spec_with_backend(
        &self,
        run_id: String,
        spec: GraphSpec,
        backend: Arc<dyn ExistingSubagentBackend>,
    ) -> String {
        let parent_session_id = match self.require_session_id() {
            Ok(session_id) => session_id.to_string(),
            Err(err) => return err,
        };
        let cwd = self.cwd.clone();
        let store_path = self.store_path.clone();
        let models_manager = self.models_manager.clone();
        let task_run_id = run_id.clone();
        let task_name = format!("agentgraph-run-{run_id}");
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle,
            Err(err) => {
                return format!(
                    "Graph run {run_id} was not started: async runtime unavailable: {err}"
                );
            }
        };
        let spawn_result = std::thread::Builder::new().name(task_name).spawn(move || {
            handle.block_on(async move {
                let mut store = match AgentGraphStore::open(&store_path) {
                    Ok(store) => store,
                    Err(err) => {
                        tracing::error!(
                            run_id = %task_run_id,
                            "failed to open AgentGraph store for background run: {err}"
                        );
                        return;
                    }
                };
                let config = SchedulerConfig::from_graph(&spec);
                let mut scheduler = match AgentGraphScheduler::new(spec, config) {
                    Ok(scheduler) => scheduler,
                    Err(err) => {
                        let _ = store.mark_run_status(&task_run_id, RunStatus::Failed);
                        tracing::error!(
                            run_id = %task_run_id,
                            "failed to build AgentGraph scheduler: {err}"
                        );
                        return;
                    }
                };
                let mut worker_backend =
                    SubagentWorkerBackend::new(backend, parent_session_id, cwd);
                if let Some(models_manager) = models_manager {
                    worker_backend = worker_backend.with_models_manager(models_manager);
                }
                if let Err(err) = scheduler
                    .run_to_completion_with_subagents(&worker_backend, &mut store, &task_run_id)
                    .await
                {
                    let _ = store.mark_run_status(&task_run_id, RunStatus::Failed);
                    tracing::error!(
                        run_id = %task_run_id,
                        "AgentGraph background run failed: {err}"
                    );
                }
            });
        });
        if let Err(err) = spawn_result {
            let _ = AgentGraphStore::open(&self.store_path)
                .and_then(|mut store| store.mark_run_status(&run_id, RunStatus::Failed));
            return format!(
                "Graph run {run_id} was not started: failed to spawn background task: {err}"
            );
        }

        format!(
            "Graph run {run_id} started.\nStatus: Running\nBackend: subagent\nUse `/graph status` to monitor it, and `/graph pause`, `/graph resume`, `/graph drain`, or `/graph cancel` to request lifecycle changes."
        )
    }

    fn active_run_id(&self) -> Result<Option<String>, String> {
        let session_id = self.require_session_id()?;
        let store = AgentGraphStore::open(&self.store_path)
            .map_err(|err| format!("Failed to open AgentGraph store: {err}"))?;
        store
            .active_run_for_session(session_id, &self.cwd)
            .map_err(|err| format!("Failed to load active graph run for this session: {err}"))
    }

    fn require_session_id(&self) -> Result<&str, String> {
        self.session_id.as_deref().ok_or_else(|| {
            "AgentGraph commands require a session id so active runs do not collide across sessions."
                .to_string()
        })
    }
}

pub fn graph_command_output(args: &str, cwd: &Path, session_id: Option<&str>) -> String {
    let trimmed = args.trim();
    let control = AgentGraphControlPlane::new(cwd, session_id);
    if trimmed.is_empty() || trimmed == "status" {
        return control.status("AgentGraph");
    }
    if let Some(path) = trimmed.strip_prefix("validate ") {
        return validate_graph_file(cwd, path.trim(), false);
    }
    if let Some(path) = trimmed.strip_prefix("preview ") {
        return validate_graph_file(cwd, path.trim(), true);
    }
    if let Some(path) = trimmed.strip_prefix("plan ") {
        let spec = match load_graph_spec(cwd, path.trim()) {
            Ok(spec) => spec,
            Err(err) => return err,
        };
        return control.create_run_from_spec(&spec);
    }
    if trimmed == "approval" {
        return control.approval_material();
    }
    if let Some(hash) = trimmed.strip_prefix("approve ") {
        return control.approve_active(hash.trim());
    }
    if let Some(path) = trimmed.strip_prefix("run ") {
        let spec = match load_graph_spec(cwd, path.trim()) {
            Ok(spec) => spec,
            Err(err) => return err,
        };
        return control.run_spec_now(spec);
    }
    match trimmed {
        "run" => return control.run_active(),
        "pause" => return control.transition_active(RunStatus::Paused, "pause"),
        "drain" => return control.transition_active(RunStatus::Draining, "drain"),
        "resume" => return control.transition_active(RunStatus::Running, "resume"),
        "cancel" => return control.transition_active(RunStatus::Cancelled, "cancel"),
        "retry-failed" => {
            return control.transition_active(RunStatus::Running, "retry failed nodes");
        }
        _ => {}
    }
    "Usage: /graph status | validate <path> | preview <path> | plan <path> | approval | approve <hash> | run [path] | pause | drain | resume | cancel | retry-failed".to_string()
}

pub async fn graph_command_output_with_backend(
    args: &str,
    cwd: &Path,
    session_id: Option<&str>,
    backend: Option<Arc<dyn ExistingSubagentBackend>>,
    models_manager: Option<crate::agent::models::ModelsManager>,
) -> String {
    let trimmed = args.trim();
    let mut control = AgentGraphControlPlane::new(cwd, session_id);
    if let Some(models_manager) = models_manager {
        control = control.with_models_manager(models_manager);
    }
    if let Some(path) = trimmed.strip_prefix("run ") {
        let spec = match load_graph_spec(cwd, path.trim()) {
            Ok(spec) => spec,
            Err(err) => return err,
        };
        return match backend {
            Some(backend) => control.run_spec_now_with_backend(spec, backend),
            None => control.run_spec_now(spec),
        };
    }
    if trimmed == "run" {
        return match backend {
            Some(backend) => control.run_active_with_backend(backend),
            None => control.run_active(),
        };
    }
    graph_command_output(args, cwd, session_id)
}

pub fn swarm_command_output(args: &str, cwd: &Path, session_id: Option<&str>) -> String {
    let trimmed = args.trim();
    let control = AgentGraphControlPlane::new(cwd, session_id);
    if trimmed.is_empty() || trimmed == "status" {
        return control.status("Swarm");
    }
    if let Some(objective) = trimmed.strip_prefix("plan ")
        && objective.trim() != "--fake"
    {
        let objective = objective.trim();
        if objective.is_empty() {
            return "Usage: /swarm plan <objective>".to_string();
        }
        let mut spec = build_exact_100_worker_graph("exact-100-swarm-plan");
        spec.spec.objective = objective.to_string();
        return control.create_run_from_spec(&spec);
    }
    if trimmed == "plan" || trimmed == "plan --fake" {
        let spec = build_exact_100_worker_graph("exact-100-swarm-plan");
        return control.create_run_from_spec(&spec);
    }
    if trimmed == "preview" || trimmed == "preview --fake" {
        let spec = build_exact_100_worker_graph("exact-100-swarm-preview");
        return preview_valid_graph(&spec);
    }
    if trimmed == "run" {
        return "Swarm run was not executed.\nExecution backend: unavailable for this command surface. Use `/swarm run --fake` for the offline scheduler benchmark path.".to_string();
    }
    if trimmed == "run --fake" {
        let spec = build_exact_100_worker_graph("exact-100-swarm-run");
        return control.run_fake_spec_now(spec);
    }
    match trimmed {
        "pause" => return control.transition_active(RunStatus::Paused, "pause"),
        "drain" => return control.transition_active(RunStatus::Draining, "drain"),
        "resume" => return control.transition_active(RunStatus::Running, "resume"),
        "cancel" => return control.transition_active(RunStatus::Cancelled, "cancel"),
        _ => {}
    }
    if let Some(rest) = trimmed.strip_prefix("benchmark") {
        if !rest.contains("--fake") {
            return "Usage: /swarm benchmark --fake [--limit <n>]\nLive provider benchmarks are intentionally not run by this command.".to_string();
        }
        let limit = parse_limit(rest).unwrap_or(100);
        return match run_exact_100_fake_benchmark(limit) {
            Ok(report) => format!(
                "Exact-100 fake benchmark passed.\nWorkers: {}\nConfigured limit: {}\nPeak active: {}\nQueued by cap: {}\nCompleted: {}\nFailed: {}\nTimed out: {}\nCancelled: {}\nDuration: {} ms\nBackend: fake",
                report.total_worker_nodes,
                report.configured_limit,
                report.peak_active_workers,
                report.queued_workers,
                report.completed_workers,
                report.failed_workers,
                report.timed_out_workers,
                report.cancelled_workers,
                report.duration_ms,
            ),
            Err(err) => format!("Exact-100 fake benchmark failed: {err}"),
        };
    }
    if let Some(path) = trimmed.strip_prefix("validate ") {
        return validate_graph_file(cwd, path.trim(), true);
    }
    "Usage: /swarm status | plan [--fake] | preview [--fake] | run [--fake] | pause | drain | resume | cancel | benchmark --fake [--limit <n>] | validate <path>".to_string()
}

pub async fn swarm_command_output_with_backend(
    args: &str,
    cwd: &Path,
    session_id: Option<&str>,
    backend: Option<Arc<dyn ExistingSubagentBackend>>,
    live_enabled: bool,
    models_manager: Option<crate::agent::models::ModelsManager>,
) -> String {
    let trimmed = args.trim();
    let mut control = AgentGraphControlPlane::new(cwd, session_id);
    if let Some(models_manager) = models_manager {
        control = control.with_models_manager(models_manager);
    }
    if trimmed == "run" {
        if !live_enabled {
            return "Live Swarm is disabled. Set `[orchestration].live_swarm_enabled = true`, keep a hard budget, and approve the current normalized graph before retrying.".to_string();
        }
        return match backend {
            Some(backend) => control.run_active_with_backend(backend),
            None => {
                "Live Swarm preflight failed: this session has no provider-backed worker backend."
                    .to_string()
            }
        };
    }
    if trimmed == "approval" {
        return control.approval_material();
    }
    if let Some(hash) = trimmed.strip_prefix("approve ") {
        return control.approve_active(hash.trim());
    }
    swarm_command_output(args, cwd, session_id)
}

fn validate_graph_file(cwd: &Path, path: &str, preview: bool) -> String {
    if path.is_empty() {
        return "Usage: /graph validate <path>".to_string();
    }
    let spec = match load_graph_spec(cwd, path) {
        Ok(spec) => spec,
        Err(err) => return err,
    };
    validate_or_preview_graph(&spec, preview)
}

fn load_graph_spec(cwd: &Path, path: &str) -> Result<GraphSpec, String> {
    if path.is_empty() {
        return Err("Usage: /graph validate <path>".to_string());
    }
    let full_path = resolve_path(cwd, path);
    let raw = std::fs::read_to_string(&full_path)
        .map_err(|err| format!("Failed to read {}: {err}", full_path.display()))?;
    parse_graph_spec(&raw, &full_path)
        .map_err(|err| format!("Failed to parse {}: {err}", full_path.display()))
}

fn validate_or_preview_graph(spec: &GraphSpec, preview: bool) -> String {
    let report = validate_graph_spec(&spec, &ValidationOptions::default());
    if !report.is_valid() {
        let errors = report
            .errors
            .iter()
            .take(8)
            .map(|err| format!("  - {err}"))
            .collect::<Vec<_>>()
            .join("\n");
        return format!(
            "GraphSpec validation failed with {} error(s):\n{}",
            report.errors.len(),
            errors
        );
    }
    if preview {
        return preview_valid_graph(spec);
    }
    format!(
        "GraphSpec is valid.\nHash: {}",
        report
            .normalized_hash
            .unwrap_or_else(|| "<unavailable>".to_string())
    )
}

fn validate_graph_for_command(spec: &GraphSpec) -> Result<(), String> {
    let report = validate_graph_spec(spec, &ValidationOptions::default());
    if report.is_valid() {
        Ok(())
    } else {
        Err(format!(
            "GraphSpec validation failed with {} error(s). Use `/graph preview <path>` for details.",
            report.errors.len()
        ))
    }
}

fn preview_valid_graph(spec: &GraphSpec) -> String {
    let report = validate_graph_spec(spec, &ValidationOptions::default());
    if !report.is_valid() {
        return validate_or_preview_graph(spec, false);
    }
    let topology = report.topology;
    let topology_lines = topology.map_or_else(
        || "Topology: unavailable".to_string(),
        |topology| {
            format!(
                "Nodes: {}\nEdges: {}\nInitial ready agents: {}\nMaximum theoretical width: {}\nCritical path length: {}",
                topology.node_count,
                topology.edge_count,
                topology.initial_ready_agent_width,
                topology.maximum_theoretical_width,
                topology.critical_path_length
            )
        },
    );
    let defaults = &spec.spec.defaults;
    let effects = spec
        .spec
        .nodes
        .iter()
        .map(|node| node.external_effects.len())
        .sum::<usize>();
    let write_capable = spec
        .spec
        .nodes
        .iter()
        .filter(|node| node.effective_capability(defaults) > super::types::CapabilityMode::ReadOnly)
        .count();
    format!(
        "GraphSpec is valid.\nHash: {}\n{}\nBudget: wall={:?}, input={:?}, output={:?}, cost={:?}\nModels: defaults={:?}, selectors={:?}\nPermissions: default={:?}, write_capable_nodes={}, nested_agents_disabled={}, provider_multi_agent_disabled={}\nEffects: external_effects={}, resources={}",
        report
            .normalized_hash
            .unwrap_or_else(|| "<unavailable>".to_string()),
        topology_lines,
        spec.spec.budgets.max_wall_time_seconds,
        spec.spec.budgets.max_input_tokens,
        spec.spec.budgets.max_output_tokens,
        spec.spec.budgets.max_estimated_cost_usd,
        defaults.model_selector,
        spec.spec.model_selectors,
        defaults.capability_mode,
        write_capable,
        spec.spec.execution.disable_nested_bandicot_agents,
        spec.spec.execution.disable_provider_multi_agent_for_workers,
        effects,
        spec.spec.resources.len()
    )
}

fn parse_graph_spec(raw: &str, path: &Path) -> Result<GraphSpec, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "yaml" | "yml") {
        serde_yaml::from_str(raw).map_err(|err| err.to_string())
    } else {
        serde_json::from_str(raw)
            .or_else(|_| serde_yaml::from_str(raw))
            .map_err(|err| err.to_string())
    }
}

fn resolve_path(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn parse_limit(args: &str) -> Option<u32> {
    let mut parts = args.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "--limit" {
            return parts.next().and_then(|value| value.parse().ok());
        }
        if let Some(value) = part.strip_prefix("--limit=") {
            return value.parse().ok();
        }
    }
    None
}
