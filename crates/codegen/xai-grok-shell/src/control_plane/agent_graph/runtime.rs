use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::retry::{FailureClassification, RetryDecision, decide_retry};
use super::store::{AgentGraphStore, GraphEvent, StoreError, now_ms};
use super::types::{CapabilityMode, IdempotencyPolicy, NodeStatus, RunStatus};

const COORDINATOR_TTL_MS: i64 = 15_000;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const SWEEP_INTERVAL: Duration = Duration::from_millis(500);

pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<std::sync::atomic::AtomicI64>,
}

impl ManualClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now: Arc::new(std::sync::atomic::AtomicI64::new(now_ms)),
        }
    }

    pub fn set(&self, now_ms: i64) {
        self.now.store(now_ms, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: i64) {
        self.now
            .fetch_add(delta_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    pub acquired_coordinator: bool,
    pub inspected_runs: usize,
    pub stale_attempts: usize,
    pub retries_scheduled: usize,
    pub retries_activated: usize,
    pub wall_time_stops: usize,
    pub manual_interventions: usize,
}

#[derive(Debug, Error)]
pub enum RuntimeManagerError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
}

#[derive(Clone)]
pub struct AgentGraphRuntimeManager {
    db_path: PathBuf,
    scope: String,
    owner: String,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AgentGraphRuntimeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentGraphRuntimeManager")
            .field("db_path", &self.db_path)
            .field("scope", &self.scope)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl AgentGraphRuntimeManager {
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self::with_clock(db_path, Arc::new(SystemClock))
    }

    pub fn with_clock(db_path: impl AsRef<Path>, clock: Arc<dyn Clock>) -> Self {
        let db_path = db_path.as_ref().to_path_buf();
        Self {
            scope: db_path.display().to_string(),
            db_path,
            owner: format!("{}-{}", std::process::id(), Uuid::new_v4()),
            clock,
        }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn recover_once(&self) -> Result<RecoveryReport, RuntimeManagerError> {
        let at_ms = self.clock.now_ms();
        let mut store = AgentGraphStore::open(&self.db_path)?;
        let acquired =
            store.try_acquire_coordinator(&self.scope, &self.owner, at_ms, COORDINATOR_TTL_MS)?;
        if !acquired {
            return Ok(RecoveryReport::default());
        }

        let mut report = RecoveryReport {
            acquired_coordinator: true,
            ..RecoveryReport::default()
        };
        for run in store.nonterminal_runs()? {
            report.inspected_runs += 1;
            let spec = store.graph_spec_for_run(&run.run_id)?;
            if spec
                .spec
                .budgets
                .max_wall_time_seconds
                .is_some_and(|seconds| {
                    at_ms
                        >= run
                            .created_at_ms
                            .saturating_add(seconds.saturating_mul(1_000) as i64)
                })
            {
                store.mark_run_status(&run.run_id, RunStatus::BudgetStopped)?;
                report.wall_time_stops += 1;
                continue;
            }

            let snapshot = store.replay_run(&run.run_id)?;
            for state in snapshot
                .node_states
                .iter()
                .filter(|state| state.status == NodeStatus::Leased)
            {
                store.append_event(
                    &run.run_id,
                    GraphEvent::LeaseExpired {
                        node_id: state.node_id.clone(),
                        attempt: state.attempt,
                    },
                )?;
                report.stale_attempts += 1;
                let Some(node) = spec.spec.nodes.iter().find(|node| node.id == state.node_id)
                else {
                    store.mark_run_status(&run.run_id, RunStatus::ManualInterventionRequired)?;
                    report.manual_interventions += 1;
                    continue;
                };
                let safe_to_retry = node.effective_capability(&spec.spec.defaults)
                    == CapabilityMode::ReadOnly
                    || node.idempotency_policy == IdempotencyPolicy::Idempotent;
                if !safe_to_retry {
                    store.mark_run_status(&run.run_id, RunStatus::ManualInterventionRequired)?;
                    report.manual_interventions += 1;
                    continue;
                }
                let (delay, equivalent, no_progress) =
                    store.retry_history_totals(&run.run_id, &state.node_id)?;
                if let RetryDecision::Schedule(schedule) = decide_retry(
                    &run.run_id,
                    node,
                    state.attempt,
                    FailureClassification::WorkerProcessFailed,
                    at_ms,
                    run.created_at_ms,
                    delay,
                    equivalent,
                    no_progress,
                    None,
                ) {
                    store.schedule_retry(&schedule)?;
                    report.retries_scheduled += 1;
                } else {
                    store.mark_run_status(&run.run_id, RunStatus::PartiallyCompleted)?;
                }
            }
            report.retries_activated += store
                .activate_due_retries_for_run(&run.run_id, at_ms)?
                .len();
            store.append_event(
                &run.run_id,
                GraphEvent::RunRecovered {
                    coordinator_owner: self.owner.clone(),
                },
            )?;
        }
        Ok(report)
    }

    pub fn heartbeat(&self) -> Result<bool, RuntimeManagerError> {
        let at_ms = self.clock.now_ms();
        let mut store = AgentGraphStore::open(&self.db_path)?;
        Ok(store.heartbeat_coordinator(&self.scope, &self.owner, at_ms, COORDINATOR_TTL_MS)?)
    }

    pub fn start(self: Arc<Self>, cancellation: CancellationToken) {
        tokio::spawn(async move {
            let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
            let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    _ = heartbeat.tick() => {
                        if !self.heartbeat().unwrap_or(false) {
                            let _ = self.recover_once();
                        }
                    }
                    _ = sweep.tick() => {
                        let _ = self.recover_once();
                    }
                }
            }
        });
    }
}

#[derive(Clone)]
struct RuntimeRegistration {
    _manager: Arc<AgentGraphRuntimeManager>,
    cancellation: CancellationToken,
}

static RUNTIME_MANAGERS: OnceLock<Mutex<BTreeMap<PathBuf, RuntimeRegistration>>> = OnceLock::new();

pub fn ensure_runtime_manager(repo_root: impl AsRef<Path>) {
    let repo_root = repo_root.as_ref();
    let db_path = repo_root.join(".agent").join("agentgraph.db");
    let registry = RUNTIME_MANAGERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut registry) = registry.lock() else {
        return;
    };
    if registry.contains_key(&db_path) {
        return;
    }
    let manager = Arc::new(AgentGraphRuntimeManager::new(&db_path));
    let cancellation = CancellationToken::new();
    let _ = manager.recover_once();
    manager.clone().start(cancellation.clone());
    registry.insert(
        db_path,
        RuntimeRegistration {
            _manager: manager,
            cancellation,
        },
    );
}

pub fn stop_runtime_manager(repo_root: impl AsRef<Path>) {
    let db_path = repo_root.as_ref().join(".agent").join("agentgraph.db");
    if let Some(registry) = RUNTIME_MANAGERS.get()
        && let Ok(mut registry) = registry.lock()
        && let Some(registration) = registry.remove(&db_path)
    {
        registration.cancellation.cancel();
    }
}
