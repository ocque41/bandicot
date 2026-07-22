use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use xai_sqlite_journal::JournalMode;

use super::approval::ExecutionApproval;
use super::budget::{BudgetError, BudgetLedger, BudgetPersistentState, BudgetReservationId};
use super::compensation::{CompensationPlan, CompensationPlanStatus};
use super::loop_controller::{LoopState, LoopStatus};
use super::normalization::canonical_graph_hash;
use super::retry::RetrySchedule;
use super::types::{GraphSpec, NodeId, NodeOutput, NodeStatus, RunStatus, UsageAccounting};

pub const STORE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("normalization error: {0}")]
    Normalization(#[from] super::normalization::NormalizationError),
    #[error("run `{run_id}` was not found")]
    RunNotFound { run_id: String },
    #[error("node `{node_id}` in run `{run_id}` was not found")]
    NodeNotFound { run_id: String, node_id: NodeId },
    #[error("budget error: {0}")]
    Budget(#[from] BudgetError),
    #[error("AgentGraph store schema {found} is newer than supported schema {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GraphEvent {
    RunCreated {
        status: RunStatus,
    },
    RunStatusChanged {
        status: RunStatus,
    },
    NodeStatusChanged {
        node_id: NodeId,
        status: NodeStatus,
    },
    NodeLeased {
        node_id: NodeId,
        attempt: u32,
        owner: String,
        lease_expires_at_ms: i64,
    },
    NodeOutputAccepted {
        node_id: NodeId,
        attempt: u32,
        status: NodeStatus,
    },
    LeaseExpired {
        node_id: NodeId,
        attempt: u32,
    },
    RetryScheduled {
        schedule: RetrySchedule,
    },
    RetryReady {
        node_id: NodeId,
        next_attempt: u32,
    },
    RunCancelled {
        reason: String,
    },
    RunRecovered {
        coordinator_owner: String,
    },
    LoopStateUpdated {
        node_id: NodeId,
        iteration: u32,
        status: LoopStatus,
    },
    CompensationUpdated {
        status: CompensationPlanStatus,
        cursor: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLease {
    pub run_id: String,
    pub node_id: NodeId,
    pub attempt: u32,
    pub owner: String,
    pub lease_expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseReservationOutcome {
    Leased(NodeLease),
    NotRunnable,
    BudgetStopped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableRun {
    pub run_id: String,
    pub status: RunStatus,
    pub session_id: Option<String>,
    pub repo_root: PathBuf,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorLease {
    pub scope: String,
    pub owner: String,
    pub expires_at_ms: i64,
    pub heartbeat_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphRunSnapshot {
    pub run_id: String,
    pub status: RunStatus,
    pub spec_hash: String,
    pub node_states: Vec<NodeStateSnapshot>,
    pub events: Vec<GraphEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeStateSnapshot {
    pub node_id: NodeId,
    pub status: NodeStatus,
    pub attempt: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub output: Option<NodeOutput>,
}

pub struct AgentGraphStore {
    db_path: PathBuf,
    conn: Connection,
}

impl AgentGraphStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        }
        let mode = JournalMode::for_db_path(&db_path);
        let conn = mode.open(&db_path)?;
        let store = Self { db_path, conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn create_run(
        &mut self,
        spec: &GraphSpec,
        session_id: Option<&str>,
        repo_root: &Path,
    ) -> Result<String, StoreError> {
        let run_id = Uuid::new_v4().to_string();
        self.create_run_with_id(&run_id, spec, session_id, repo_root)?;
        Ok(run_id)
    }

    pub fn create_run_with_id(
        &mut self,
        run_id: &str,
        spec: &GraphSpec,
        session_id: Option<&str>,
        repo_root: &Path,
    ) -> Result<(), StoreError> {
        let hash = canonical_graph_hash(spec)?;
        let spec_json = serde_json::to_string(spec)?;
        let now = now_ms();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO graph_specs(hash, name, spec_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![hash, spec.metadata.name, spec_json, now],
        )?;
        tx.execute(
            "INSERT INTO graph_runs(id, status, spec_hash, session_id, repo_root, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                run_id,
                status_text(RunStatus::AwaitingApproval),
                hash,
                session_id,
                repo_root.display().to_string(),
                now
            ],
        )?;
        for node in &spec.spec.nodes {
            tx.execute(
                "INSERT INTO node_instances(run_id, node_id, status, attempt, updated_at_ms)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![run_id, node.id, status_text(NodeStatus::Pending), now],
            )?;
        }
        insert_event_tx(
            &tx,
            run_id,
            &GraphEvent::RunCreated {
                status: RunStatus::AwaitingApproval,
            },
            now,
        )?;
        tx.execute(
            "INSERT INTO run_budget_state(run_id, state_json, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![
                run_id,
                serde_json::to_string(&BudgetPersistentState::default())?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_event(&mut self, run_id: &str, event: GraphEvent) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        let now = now_ms();
        let tx = self.conn.transaction()?;
        insert_event_tx(&tx, run_id, &event, now)?;
        apply_event_tx(&tx, run_id, &event, now)?;
        tx.commit()?;
        Ok(())
    }

    pub fn run_status(&self, run_id: &str) -> Result<RunStatus, StoreError> {
        self.conn
            .query_row(
                "SELECT status FROM graph_runs WHERE id = ?1",
                params![run_id],
                |row| parse_run_status(row.get::<_, String>(0)?),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })
    }

    pub fn attach_active_run(
        &mut self,
        session_id: &str,
        repo_root: &Path,
        run_id: &str,
    ) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO active_graph_runs(session_id, repo_root, run_id, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, repo_root)
             DO UPDATE SET run_id = excluded.run_id, updated_at_ms = excluded.updated_at_ms",
            params![session_id, repo_root.display().to_string(), run_id, now],
        )?;
        Ok(())
    }

    pub fn active_run_for_session(
        &self,
        session_id: &str,
        repo_root: &Path,
    ) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT run_id FROM active_graph_runs
                 WHERE session_id = ?1 AND repo_root = ?2",
                params![session_id, repo_root.display().to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn graph_spec_for_run(&self, run_id: &str) -> Result<GraphSpec, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT graph_specs.spec_json
                 FROM graph_runs
                 JOIN graph_specs ON graph_specs.hash = graph_runs.spec_hash
                 WHERE graph_runs.id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn node_status(&self, run_id: &str, node_id: &str) -> Result<NodeStatus, StoreError> {
        self.conn
            .query_row(
                "SELECT status FROM node_instances WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| parse_node_status(row.get::<_, String>(0)?),
            )
            .optional()?
            .ok_or_else(|| StoreError::NodeNotFound {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })
    }

    pub fn mark_run_status(&mut self, run_id: &str, status: RunStatus) -> Result<(), StoreError> {
        self.append_event(run_id, GraphEvent::RunStatusChanged { status })
    }

    pub fn mark_node_status(
        &mut self,
        run_id: &str,
        node_id: &str,
        status: NodeStatus,
    ) -> Result<(), StoreError> {
        self.append_event(
            run_id,
            GraphEvent::NodeStatusChanged {
                node_id: node_id.to_string(),
                status,
            },
        )
    }

    pub fn lease_node(
        &mut self,
        run_id: &str,
        node_id: &str,
        owner: &str,
        ttl_ms: i64,
    ) -> Result<Option<NodeLease>, StoreError> {
        let now = now_ms();
        let current = self
            .conn
            .query_row(
                "SELECT status, attempt FROM node_instances WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| {
                    Ok((
                        parse_node_status(row.get::<_, String>(0)?)?,
                        row.get::<_, u32>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NodeNotFound {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })?;

        if !matches!(
            current.0,
            NodeStatus::Pending
                | NodeStatus::Blocked
                | NodeStatus::Ready
                | NodeStatus::Retrying
                | NodeStatus::Stale
        ) {
            return Ok(None);
        }

        let attempt = current.1 + 1;
        let lease_expires_at_ms = now.saturating_add(ttl_ms.max(1));
        let event = GraphEvent::NodeLeased {
            node_id: node_id.to_string(),
            attempt,
            owner: owner.to_string(),
            lease_expires_at_ms,
        };
        let tx = self.conn.transaction()?;
        insert_event_tx(&tx, run_id, &event, now)?;
        apply_event_tx(&tx, run_id, &event, now)?;
        tx.commit()?;
        Ok(Some(NodeLease {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            attempt,
            owner: owner.to_string(),
            lease_expires_at_ms,
        }))
    }

    pub fn reserve_and_lease_node(
        &mut self,
        run_id: &str,
        node_id: &str,
        owner: &str,
        ttl_ms: i64,
        estimate: UsageAccounting,
    ) -> Result<LeaseReservationOutcome, StoreError> {
        let at_ms = now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                "SELECT status, attempt FROM node_instances WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| {
                    Ok((
                        parse_node_status(row.get::<_, String>(0)?)?,
                        row.get::<_, u32>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NodeNotFound {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })?;
        if !matches!(
            current.0,
            NodeStatus::Pending
                | NodeStatus::Blocked
                | NodeStatus::Ready
                | NodeStatus::Retrying
                | NodeStatus::Stale
        ) {
            return Ok(LeaseReservationOutcome::NotRunnable);
        }

        let spec_raw: String = tx.query_row(
            "SELECT graph_specs.spec_json FROM graph_runs
             JOIN graph_specs ON graph_specs.hash = graph_runs.spec_hash
             WHERE graph_runs.id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        let spec: GraphSpec = serde_json::from_str(&spec_raw)?;
        let state_raw: String = tx.query_row(
            "SELECT state_json FROM run_budget_state WHERE run_id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        let state: BudgetPersistentState = serde_json::from_str(&state_raw)?;
        let mut ledger = BudgetLedger::from_persistent_state(spec.spec.budgets, state);
        let attempt = current.1.saturating_add(1);
        let reservation_id = BudgetReservationId {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            attempt,
        };
        if let Err(error @ (BudgetError::Exceeded { .. } | BudgetError::Stopped(_))) =
            ledger.reserve(reservation_id.clone(), estimate.clone())
        {
            let reason = error.to_string();
            ledger.stop(reason.clone());
            tx.execute(
                "UPDATE run_budget_state SET state_json = ?1, updated_at_ms = ?2 WHERE run_id = ?3",
                params![
                    serde_json::to_string(&ledger.persistent_state())?,
                    at_ms,
                    run_id
                ],
            )?;
            let event = GraphEvent::RunStatusChanged {
                status: RunStatus::BudgetStopped,
            };
            insert_event_tx(&tx, run_id, &event, at_ms)?;
            apply_event_tx(&tx, run_id, &event, at_ms)?;
            tx.commit()?;
            return Ok(LeaseReservationOutcome::BudgetStopped { reason });
        }
        tx.execute(
            "UPDATE run_budget_state SET state_json = ?1, updated_at_ms = ?2 WHERE run_id = ?3",
            params![
                serde_json::to_string(&ledger.persistent_state())?,
                at_ms,
                run_id
            ],
        )?;
        tx.execute(
            "INSERT INTO budget_reservations(run_id, node_id, attempt, usage_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id,
                node_id,
                attempt,
                serde_json::to_string(&estimate)?,
                at_ms
            ],
        )?;
        let lease_expires_at_ms = at_ms.saturating_add(ttl_ms.max(1));
        let event = GraphEvent::NodeLeased {
            node_id: node_id.to_string(),
            attempt,
            owner: owner.to_string(),
            lease_expires_at_ms,
        };
        insert_event_tx(&tx, run_id, &event, at_ms)?;
        apply_event_tx(&tx, run_id, &event, at_ms)?;
        tx.commit()?;
        Ok(LeaseReservationOutcome::Leased(NodeLease {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            attempt,
            owner: owner.to_string(),
            lease_expires_at_ms,
        }))
    }

    pub fn accept_output_and_reconcile(
        &mut self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        output: &NodeOutput,
        actual_usage: Option<&UsageAccounting>,
    ) -> Result<bool, StoreError> {
        let at_ms = now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_attempt = tx
            .query_row(
                "SELECT attempt FROM node_instances WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| row.get::<_, u32>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NodeNotFound {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })?;
        if current_attempt != attempt {
            return Ok(false);
        }
        let spec_raw: String = tx.query_row(
            "SELECT graph_specs.spec_json FROM graph_runs
             JOIN graph_specs ON graph_specs.hash = graph_runs.spec_hash
             WHERE graph_runs.id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        let spec: GraphSpec = serde_json::from_str(&spec_raw)?;
        let state_raw: String = tx.query_row(
            "SELECT state_json FROM run_budget_state WHERE run_id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        let mut ledger = BudgetLedger::from_persistent_state(
            spec.spec.budgets,
            serde_json::from_str(&state_raw)?,
        );
        let reservation_id = BudgetReservationId {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            attempt,
        };
        let charged = ledger.reconcile(&reservation_id, actual_usage.cloned())?;
        tx.execute(
            "UPDATE run_budget_state SET state_json = ?1, updated_at_ms = ?2 WHERE run_id = ?3",
            params![
                serde_json::to_string(&ledger.persistent_state())?,
                at_ms,
                run_id
            ],
        )?;
        tx.execute(
            "DELETE FROM budget_reservations WHERE run_id = ?1 AND node_id = ?2 AND attempt = ?3",
            params![run_id, node_id, attempt],
        )?;
        tx.execute(
            "INSERT INTO usage_records_v2(run_id, node_id, attempt, usage_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(run_id, node_id, attempt)
             DO UPDATE SET usage_json = excluded.usage_json",
            params![
                run_id,
                node_id,
                attempt,
                serde_json::to_string(&charged)?,
                at_ms
            ],
        )?;
        let event = GraphEvent::NodeOutputAccepted {
            node_id: node_id.to_string(),
            attempt,
            status: output.status,
        };
        insert_event_tx(&tx, run_id, &event, at_ms)?;
        tx.execute(
            "UPDATE node_instances
             SET status = ?1, output_json = ?2, lease_owner = NULL,
                 lease_expires_at_ms = NULL, updated_at_ms = ?3
             WHERE run_id = ?4 AND node_id = ?5 AND attempt = ?6",
            params![
                status_text(output.status),
                serde_json::to_string(output)?,
                at_ms,
                run_id,
                node_id,
                attempt
            ],
        )?;
        tx.execute(
            "DELETE FROM leases WHERE run_id = ?1 AND node_id = ?2 AND attempt = ?3",
            params![run_id, node_id, attempt],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn budget_state(&self, run_id: &str) -> Result<BudgetPersistentState, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT state_json FROM run_budget_state WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save_loop_state(
        &mut self,
        run_id: &str,
        node_id: &str,
        state: &LoopState,
    ) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        let at_ms = now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO loop_states(run_id, node_id, state_json, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run_id, node_id) DO UPDATE SET
               state_json = excluded.state_json,
               updated_at_ms = excluded.updated_at_ms",
            params![run_id, node_id, serde_json::to_string(state)?, at_ms],
        )?;
        let event = GraphEvent::LoopStateUpdated {
            node_id: node_id.to_string(),
            iteration: state.iteration,
            status: state.status,
        };
        insert_event_tx(&tx, run_id, &event, at_ms)?;
        apply_event_tx(&tx, run_id, &event, at_ms)?;
        tx.commit()?;
        Ok(())
    }

    pub fn loop_state(&self, run_id: &str, node_id: &str) -> Result<Option<LoopState>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT state_json FROM loop_states WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        raw.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn save_compensation_plan(
        &mut self,
        run_id: &str,
        plan: &CompensationPlan,
    ) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        let at_ms = now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO compensation_plans(run_id, plan_json, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(run_id) DO UPDATE SET
               plan_json = excluded.plan_json,
               updated_at_ms = excluded.updated_at_ms",
            params![run_id, serde_json::to_string(plan)?, at_ms],
        )?;
        let event = GraphEvent::CompensationUpdated {
            status: plan.status,
            cursor: plan.cursor,
        };
        insert_event_tx(&tx, run_id, &event, at_ms)?;
        apply_event_tx(&tx, run_id, &event, at_ms)?;
        tx.commit()?;
        Ok(())
    }

    pub fn compensation_plan(&self, run_id: &str) -> Result<Option<CompensationPlan>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT plan_json FROM compensation_plans WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        raw.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn accept_output(
        &mut self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        output: &NodeOutput,
    ) -> Result<bool, StoreError> {
        let current_attempt = self
            .conn
            .query_row(
                "SELECT attempt FROM node_instances WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| row.get::<_, u32>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NodeNotFound {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })?;
        if current_attempt != attempt {
            return Ok(false);
        }

        let now = now_ms();
        let output_json = serde_json::to_string(output)?;
        let event = GraphEvent::NodeOutputAccepted {
            node_id: node_id.to_string(),
            attempt,
            status: output.status,
        };
        let tx = self.conn.transaction()?;
        insert_event_tx(&tx, run_id, &event, now)?;
        tx.execute(
            "UPDATE node_instances
             SET status = ?1, output_json = ?2, lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?3
             WHERE run_id = ?4 AND node_id = ?5",
            params![status_text(output.status), output_json, now, run_id, node_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn record_usage(
        &mut self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        usage: &UsageAccounting,
    ) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        self.conn.execute(
            "INSERT INTO usage_records_v2(
               run_id, node_id, attempt, usage_json, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(run_id, node_id, attempt)
             DO UPDATE SET usage_json = excluded.usage_json",
            params![
                run_id,
                node_id,
                attempt,
                serde_json::to_string(usage)?,
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn schedule_retry(&mut self, schedule: &RetrySchedule) -> Result<(), StoreError> {
        self.ensure_run(&schedule.run_id)?;
        let now = now_ms();
        let event = GraphEvent::RetryScheduled {
            schedule: schedule.clone(),
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO retry_schedules(
               run_id, node_id, prior_attempt, next_attempt, classification,
               chosen_delay_ms, next_attempt_at_ms, retry_after_ms, jitter_ms,
               equivalent_failure_count, no_progress_count, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(run_id, node_id) DO UPDATE SET
               prior_attempt = excluded.prior_attempt,
               next_attempt = excluded.next_attempt,
               classification = excluded.classification,
               chosen_delay_ms = excluded.chosen_delay_ms,
               next_attempt_at_ms = excluded.next_attempt_at_ms,
               retry_after_ms = excluded.retry_after_ms,
               jitter_ms = excluded.jitter_ms,
               equivalent_failure_count = excluded.equivalent_failure_count,
               no_progress_count = excluded.no_progress_count,
               created_at_ms = excluded.created_at_ms",
            params![
                schedule.run_id,
                schedule.node_id,
                schedule.prior_attempt,
                schedule.next_attempt,
                status_text(schedule.classification),
                schedule.chosen_delay_ms,
                schedule.next_attempt_at_ms,
                schedule.retry_after_ms,
                schedule.jitter_ms,
                schedule.equivalent_failure_count,
                schedule.no_progress_count,
                now,
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO retry_history(
               run_id, node_id, prior_attempt, schedule_json, chosen_delay_ms, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                schedule.run_id,
                schedule.node_id,
                schedule.prior_attempt,
                serde_json::to_string(schedule)?,
                schedule.chosen_delay_ms,
                now,
            ],
        )?;
        insert_event_tx(&tx, &schedule.run_id, &event, now)?;
        apply_event_tx(&tx, &schedule.run_id, &event, now)?;
        tx.commit()?;
        Ok(())
    }

    pub fn retry_schedule(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<RetrySchedule>, StoreError> {
        self.conn
            .query_row(
                "SELECT prior_attempt, next_attempt, classification, chosen_delay_ms,
                        next_attempt_at_ms, retry_after_ms, jitter_ms,
                        equivalent_failure_count, no_progress_count
                 FROM retry_schedules WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| {
                    let classification_raw: String = row.get(2)?;
                    Ok(RetrySchedule {
                        run_id: run_id.to_string(),
                        node_id: node_id.to_string(),
                        prior_attempt: row.get(0)?,
                        next_attempt: row.get(1)?,
                        classification: parse_json_enum(classification_raw, 2)?,
                        chosen_delay_ms: row.get(3)?,
                        next_attempt_at_ms: row.get(4)?,
                        retry_after_ms: row.get(5)?,
                        jitter_ms: row.get(6)?,
                        equivalent_failure_count: row.get(7)?,
                        no_progress_count: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn activate_due_retries(&mut self, at_ms: i64) -> Result<Vec<RetrySchedule>, StoreError> {
        self.activate_due_retries_matching(None, at_ms)
    }

    pub fn activate_due_retries_for_run(
        &mut self,
        run_id: &str,
        at_ms: i64,
    ) -> Result<Vec<RetrySchedule>, StoreError> {
        self.activate_due_retries_matching(Some(run_id), at_ms)
    }

    fn activate_due_retries_matching(
        &mut self,
        run_id: Option<&str>,
        at_ms: i64,
    ) -> Result<Vec<RetrySchedule>, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sql = if run_id.is_some() {
            "SELECT run_id, node_id, prior_attempt, next_attempt, classification,
                    chosen_delay_ms, next_attempt_at_ms, retry_after_ms, jitter_ms,
                    equivalent_failure_count, no_progress_count
             FROM retry_schedules WHERE run_id = ?1 AND next_attempt_at_ms <= ?2
             ORDER BY next_attempt_at_ms, node_id"
        } else {
            "SELECT run_id, node_id, prior_attempt, next_attempt, classification,
                    chosen_delay_ms, next_attempt_at_ms, retry_after_ms, jitter_ms,
                    equivalent_failure_count, no_progress_count
             FROM retry_schedules WHERE next_attempt_at_ms <= ?1
             ORDER BY next_attempt_at_ms, run_id, node_id"
        };
        let mut stmt = tx.prepare(sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            let classification_raw: String = row.get(4)?;
            Ok(RetrySchedule {
                run_id: row.get(0)?,
                node_id: row.get(1)?,
                prior_attempt: row.get(2)?,
                next_attempt: row.get(3)?,
                classification: parse_json_enum(classification_raw, 4)?,
                chosen_delay_ms: row.get(5)?,
                next_attempt_at_ms: row.get(6)?,
                retry_after_ms: row.get(7)?,
                jitter_ms: row.get(8)?,
                equivalent_failure_count: row.get(9)?,
                no_progress_count: row.get(10)?,
            })
        };
        let schedules = if let Some(run_id) = run_id {
            stmt.query_map(params![run_id, at_ms], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![at_ms], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        drop(stmt);
        for schedule in &schedules {
            let event = GraphEvent::RetryReady {
                node_id: schedule.node_id.clone(),
                next_attempt: schedule.next_attempt,
            };
            insert_event_tx(&tx, &schedule.run_id, &event, at_ms)?;
            apply_event_tx(&tx, &schedule.run_id, &event, at_ms)?;
            tx.execute(
                "DELETE FROM retry_schedules WHERE run_id = ?1 AND node_id = ?2",
                params![schedule.run_id, schedule.node_id],
            )?;
        }
        tx.commit()?;
        Ok(schedules)
    }

    pub fn next_retry_at(&self, run_id: &str) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT MIN(next_attempt_at_ms) FROM retry_schedules WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn retry_history_totals(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<(u64, u32, u32), StoreError> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(chosen_delay_ms), 0), COUNT(*), COUNT(*)
                 FROM retry_history WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(StoreError::from)
    }

    pub fn run_created_at_ms(&self, run_id: &str) -> Result<i64, StoreError> {
        self.conn
            .query_row(
                "SELECT created_at_ms FROM graph_runs WHERE id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })
    }

    pub fn nonterminal_runs(&self) -> Result<Vec<RecoverableRun>, StoreError> {
        let terminal = [
            RunStatus::Completed,
            RunStatus::PartiallyCompleted,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Compensated,
            RunStatus::CompensationFailed,
            RunStatus::ManualInterventionRequired,
        ]
        .map(status_text);
        let mut stmt = self.conn.prepare(
            "SELECT id, status, session_id, repo_root, created_at_ms, updated_at_ms
             FROM graph_runs
             WHERE status NOT IN (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ORDER BY created_at_ms, id",
        )?;
        let rows = stmt
            .query_map(
                params![
                    terminal[0],
                    terminal[1],
                    terminal[2],
                    terminal[3],
                    terminal[4],
                    terminal[5],
                    terminal[6]
                ],
                |row| {
                    Ok(RecoverableRun {
                        run_id: row.get(0)?,
                        status: parse_run_status(row.get(1)?)?,
                        session_id: row.get(2)?,
                        repo_root: PathBuf::from(row.get::<_, String>(3)?),
                        created_at_ms: row.get(4)?,
                        updated_at_ms: row.get(5)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn try_acquire_coordinator(
        &mut self,
        scope: &str,
        owner: &str,
        at_ms: i64,
        ttl_ms: i64,
    ) -> Result<bool, StoreError> {
        let expires_at_ms = at_ms.saturating_add(ttl_ms.max(1));
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "INSERT INTO coordinator_leases(scope, owner, expires_at_ms, heartbeat_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scope) DO UPDATE SET
               owner = excluded.owner,
               expires_at_ms = excluded.expires_at_ms,
               heartbeat_at_ms = excluded.heartbeat_at_ms
             WHERE coordinator_leases.owner = excluded.owner
                OR coordinator_leases.expires_at_ms <= excluded.heartbeat_at_ms",
            params![scope, owner, expires_at_ms, at_ms],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    pub fn heartbeat_coordinator(
        &mut self,
        scope: &str,
        owner: &str,
        at_ms: i64,
        ttl_ms: i64,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE coordinator_leases
             SET expires_at_ms = ?1, heartbeat_at_ms = ?2
             WHERE scope = ?3 AND owner = ?4 AND expires_at_ms > ?2",
            params![at_ms.saturating_add(ttl_ms.max(1)), at_ms, scope, owner],
        )?;
        Ok(changed == 1)
    }

    pub fn coordinator_lease(&self, scope: &str) -> Result<Option<CoordinatorLease>, StoreError> {
        self.conn
            .query_row(
                "SELECT owner, expires_at_ms, heartbeat_at_ms
                 FROM coordinator_leases WHERE scope = ?1",
                params![scope],
                |row| {
                    Ok(CoordinatorLease {
                        scope: scope.to_string(),
                        owner: row.get(0)?,
                        expires_at_ms: row.get(1)?,
                        heartbeat_at_ms: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn expire_leases(&mut self, now_ms: i64) -> Result<usize, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, node_id, attempt
             FROM node_instances
             WHERE status = ?1 AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms <= ?2",
        )?;
        let rows = stmt
            .query_map(params![status_text(NodeStatus::Leased), now_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        for (run_id, node_id, attempt) in &rows {
            self.append_event(
                run_id,
                GraphEvent::LeaseExpired {
                    node_id: node_id.clone(),
                    attempt: *attempt,
                },
            )?;
        }
        Ok(rows.len())
    }

    pub fn replay_run(&self, run_id: &str) -> Result<GraphRunSnapshot, StoreError> {
        let (status, spec_hash) = self
            .conn
            .query_row(
                "SELECT status, spec_hash FROM graph_runs WHERE id = ?1",
                params![run_id],
                |row| {
                    Ok((
                        parse_run_status(row.get::<_, String>(0)?)?,
                        row.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })?;

        let mut node_stmt = self.conn.prepare(
            "SELECT node_id, status, attempt, lease_owner, lease_expires_at_ms, output_json
             FROM node_instances
             WHERE run_id = ?1
             ORDER BY node_id",
        )?;
        let node_states = node_stmt
            .query_map(params![run_id], |row| {
                let output_json: Option<String> = row.get(5)?;
                Ok(NodeStateSnapshot {
                    node_id: row.get(0)?,
                    status: parse_node_status(row.get::<_, String>(1)?)?,
                    attempt: row.get(2)?,
                    lease_owner: row.get(3)?,
                    lease_expires_at_ms: row.get(4)?,
                    output: output_json
                        .map(|raw| serde_json::from_str(&raw))
                        .transpose()
                        .map_err(|err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Text,
                                Box::new(err),
                            )
                        })?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let events = self.events_for_run(run_id)?;
        Ok(GraphRunSnapshot {
            run_id: run_id.to_string(),
            status,
            spec_hash,
            node_states,
            events,
        })
    }

    pub fn events_for_run(&self, run_id: &str) -> Result<Vec<GraphEvent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT event_json FROM graph_events WHERE run_id = ?1 ORDER BY sequence ASC",
        )?;
        let events = stmt
            .query_map(params![run_id], |row| {
                let raw: String = row.get(0)?;
                serde_json::from_str(&raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn save_execution_approval(
        &mut self,
        run_id: &str,
        approval: &ExecutionApproval,
    ) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        self.conn.execute(
            "INSERT INTO execution_approvals(run_id, approval_json, created_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(run_id) DO UPDATE SET
               approval_json = excluded.approval_json,
               created_at_ms = excluded.created_at_ms",
            params![run_id, serde_json::to_string(approval)?, now_ms()],
        )?;
        Ok(())
    }

    pub fn execution_approval(
        &self,
        run_id: &str,
    ) -> Result<Option<ExecutionApproval>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT approval_json FROM execution_approvals WHERE run_id = ?1",
                params![run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        raw.map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(StoreError::from)
    }

    pub fn cleanup_run(&mut self, run_id: &str) -> Result<(), StoreError> {
        self.ensure_run(run_id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in [
            "active_graph_runs",
            "execution_approvals",
            "compensation_plans",
            "loop_states",
            "retry_history",
            "retry_schedules",
            "budget_reservations",
            "run_budget_state",
            "leases",
            "node_attempts",
            "node_instances",
            "graph_events",
            "artifacts",
            "usage_records",
            "usage_records_v2",
            "approvals",
            "resource_claims",
            "cache_entries",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE run_id = ?1"),
                params![run_id],
            )?;
        }
        tx.execute("DELETE FROM graph_runs WHERE id = ?1", params![run_id])?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_run(&self, run_id: &str) -> Result<(), StoreError> {
        let exists = self
            .conn
            .query_row(
                "SELECT 1 FROM graph_runs WHERE id = ?1",
                params![run_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            Ok(())
        } else {
            Err(StoreError::RunNotFound {
                run_id: run_id.to_string(),
            })
        }
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                component TEXT PRIMARY KEY,
                version INTEGER NOT NULL
            );",
        )?;
        let installed = self
            .conn
            .query_row(
                "SELECT version FROM schema_migrations WHERE component = 'agent_graph_store'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .optional()?;
        if installed.is_some_and(|found| found > STORE_SCHEMA_VERSION) {
            return Err(StoreError::UnsupportedSchema {
                found: installed.unwrap_or_default(),
                supported: STORE_SCHEMA_VERSION,
            });
        }
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_migrations(
                component TEXT PRIMARY KEY,
                version INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS graph_specs(
                hash TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                spec_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS graph_revisions(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                spec_hash TEXT NOT NULL,
                revision_json TEXT NOT NULL DEFAULT '{}',
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS graph_runs(
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                spec_hash TEXT NOT NULL,
                session_id TEXT,
                repo_root TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY(spec_hash) REFERENCES graph_specs(hash)
            );
            CREATE TABLE IF NOT EXISTS active_graph_runs(
                session_id TEXT NOT NULL,
                repo_root TEXT NOT NULL,
                run_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY(session_id, repo_root),
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            CREATE TABLE IF NOT EXISTS node_instances(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                status TEXT NOT NULL,
                attempt INTEGER NOT NULL DEFAULT 0,
                lease_owner TEXT,
                lease_expires_at_ms INTEGER,
                output_json TEXT,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id),
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            CREATE TABLE IF NOT EXISTS node_attempts(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                worker_id TEXT,
                started_at_ms INTEGER,
                finished_at_ms INTEGER,
                status TEXT,
                PRIMARY KEY(run_id, node_id, attempt)
            );
            CREATE TABLE IF NOT EXISTS graph_events(
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                event_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            CREATE TABLE IF NOT EXISTS leases(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                owner TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id)
            );
            CREATE TABLE IF NOT EXISTS artifacts(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                node_id TEXT,
                path TEXT NOT NULL,
                sha256 TEXT,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS usage_records(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                node_id TEXT,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cost_usd REAL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS usage_records_v2(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                usage_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id, attempt)
            );
            CREATE TABLE IF NOT EXISTS approvals(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                node_id TEXT,
                status TEXT NOT NULL,
                prompt TEXT,
                decided_at_ms INTEGER
            );
            CREATE TABLE IF NOT EXISTS execution_approvals(
                run_id TEXT PRIMARY KEY,
                approval_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            CREATE TABLE IF NOT EXISTS resource_claims(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                resource TEXT NOT NULL,
                amount INTEGER NOT NULL,
                acquired_at_ms INTEGER NOT NULL,
                released_at_ms INTEGER
            );
            CREATE TABLE IF NOT EXISTS cache_entries(
                cache_key TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                output_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS retry_schedules(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                prior_attempt INTEGER NOT NULL,
                next_attempt INTEGER NOT NULL,
                classification TEXT NOT NULL,
                chosen_delay_ms INTEGER NOT NULL,
                next_attempt_at_ms INTEGER NOT NULL,
                retry_after_ms INTEGER,
                jitter_ms INTEGER NOT NULL,
                equivalent_failure_count INTEGER NOT NULL,
                no_progress_count INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id),
                FOREIGN KEY(run_id, node_id) REFERENCES node_instances(run_id, node_id)
            );
            CREATE INDEX IF NOT EXISTS retry_schedules_due
                ON retry_schedules(next_attempt_at_ms);
            CREATE TABLE IF NOT EXISTS retry_history(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                prior_attempt INTEGER NOT NULL,
                schedule_json TEXT NOT NULL,
                chosen_delay_ms INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id, prior_attempt),
                FOREIGN KEY(run_id, node_id) REFERENCES node_instances(run_id, node_id)
            );
            CREATE TABLE IF NOT EXISTS coordinator_leases(
                scope TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                heartbeat_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS run_budget_state(
                run_id TEXT PRIMARY KEY,
                state_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            CREATE TABLE IF NOT EXISTS budget_reservations(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                usage_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id, attempt),
                FOREIGN KEY(run_id, node_id) REFERENCES node_instances(run_id, node_id)
            );
            CREATE TABLE IF NOT EXISTS loop_states(
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                state_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY(run_id, node_id),
                FOREIGN KEY(run_id, node_id) REFERENCES node_instances(run_id, node_id)
            );
            CREATE TABLE IF NOT EXISTS compensation_plans(
                run_id TEXT PRIMARY KEY,
                plan_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY(run_id) REFERENCES graph_runs(id)
            );
            ",
        )?;
        self.conn.execute(
            "INSERT INTO schema_migrations(component, version)
             VALUES ('agent_graph_store', ?1)
             ON CONFLICT(component) DO UPDATE SET version = excluded.version",
            params![STORE_SCHEMA_VERSION],
        )?;
        Ok(())
    }
}

fn insert_event_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    event: &GraphEvent,
    now: i64,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO graph_events(run_id, event_type, event_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            run_id,
            event_type(event),
            serde_json::to_string(event)?,
            now
        ],
    )?;
    Ok(())
}

fn apply_event_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    event: &GraphEvent,
    now: i64,
) -> Result<(), StoreError> {
    match event {
        GraphEvent::RunCreated { status } | GraphEvent::RunStatusChanged { status } => {
            tx.execute(
                "UPDATE graph_runs SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![status_text(*status), now, run_id],
            )?;
        }
        GraphEvent::NodeStatusChanged { node_id, status } => {
            tx.execute(
                "UPDATE node_instances SET status = ?1, updated_at_ms = ?2 WHERE run_id = ?3 AND node_id = ?4",
                params![status_text(*status), now, run_id, node_id],
            )?;
        }
        GraphEvent::NodeLeased {
            node_id,
            attempt,
            owner,
            lease_expires_at_ms,
        } => {
            tx.execute(
                "UPDATE node_instances
                 SET status = ?1, attempt = ?2, lease_owner = ?3, lease_expires_at_ms = ?4, updated_at_ms = ?5
                 WHERE run_id = ?6 AND node_id = ?7",
                params![
                    status_text(NodeStatus::Leased),
                    attempt,
                    owner,
                    lease_expires_at_ms,
                    now,
                    run_id,
                    node_id
                ],
            )?;
            tx.execute(
                "INSERT INTO leases(run_id, node_id, owner, attempt, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(run_id, node_id)
                 DO UPDATE SET owner = excluded.owner, attempt = excluded.attempt, expires_at_ms = excluded.expires_at_ms",
                params![run_id, node_id, owner, attempt, lease_expires_at_ms],
            )?;
        }
        GraphEvent::NodeOutputAccepted {
            node_id,
            attempt: _,
            status,
        } => {
            tx.execute(
                "UPDATE node_instances
                 SET status = ?1, lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?2
                 WHERE run_id = ?3 AND node_id = ?4",
                params![status_text(*status), now, run_id, node_id],
            )?;
            tx.execute(
                "DELETE FROM leases WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, node_id],
            )?;
        }
        GraphEvent::LeaseExpired { node_id, attempt } => {
            tx.execute(
                "UPDATE node_instances
                 SET status = ?1, lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?2
                 WHERE run_id = ?3 AND node_id = ?4 AND attempt = ?5",
                params![
                    status_text(NodeStatus::Stale),
                    now,
                    run_id,
                    node_id,
                    attempt
                ],
            )?;
            tx.execute(
                "DELETE FROM leases WHERE run_id = ?1 AND node_id = ?2 AND attempt = ?3",
                params![run_id, node_id, attempt],
            )?;
            reconcile_budget_tx(tx, run_id, node_id, *attempt, None, now)?;
        }
        GraphEvent::RetryScheduled { schedule } => {
            tx.execute(
                "UPDATE node_instances
                 SET status = ?1, lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?2
                 WHERE run_id = ?3 AND node_id = ?4 AND attempt = ?5",
                params![
                    status_text(NodeStatus::Retrying),
                    now,
                    run_id,
                    schedule.node_id,
                    schedule.prior_attempt
                ],
            )?;
            tx.execute(
                "DELETE FROM leases WHERE run_id = ?1 AND node_id = ?2",
                params![run_id, schedule.node_id],
            )?;
        }
        GraphEvent::RetryReady {
            node_id,
            next_attempt: _,
        } => {
            tx.execute(
                "UPDATE node_instances SET status = ?1, updated_at_ms = ?2
                 WHERE run_id = ?3 AND node_id = ?4 AND status = ?5",
                params![
                    status_text(NodeStatus::Ready),
                    now,
                    run_id,
                    node_id,
                    status_text(NodeStatus::Retrying)
                ],
            )?;
        }
        GraphEvent::RunCancelled { .. } => {
            tx.execute(
                "UPDATE graph_runs SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![status_text(RunStatus::Cancelled), now, run_id],
            )?;
        }
        GraphEvent::RunRecovered { .. } => {
            tx.execute(
                "UPDATE graph_runs SET updated_at_ms = ?1 WHERE id = ?2",
                params![now, run_id],
            )?;
        }
        GraphEvent::LoopStateUpdated { .. } => {}
        GraphEvent::CompensationUpdated { status, .. } => {
            let run_status = match status {
                CompensationPlanStatus::Pending | CompensationPlanStatus::Running => {
                    RunStatus::Compensating
                }
                CompensationPlanStatus::Completed => RunStatus::Compensated,
                CompensationPlanStatus::Failed => RunStatus::CompensationFailed,
                CompensationPlanStatus::ManualInterventionRequired => {
                    RunStatus::ManualInterventionRequired
                }
            };
            tx.execute(
                "UPDATE graph_runs SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![status_text(run_status), now, run_id],
            )?;
        }
    }
    Ok(())
}

fn event_type(event: &GraphEvent) -> &'static str {
    match event {
        GraphEvent::RunCreated { .. } => "run_created",
        GraphEvent::RunStatusChanged { .. } => "run_status_changed",
        GraphEvent::NodeStatusChanged { .. } => "node_status_changed",
        GraphEvent::NodeLeased { .. } => "node_leased",
        GraphEvent::NodeOutputAccepted { .. } => "node_output_accepted",
        GraphEvent::LeaseExpired { .. } => "lease_expired",
        GraphEvent::RetryScheduled { .. } => "node_retry_scheduled",
        GraphEvent::RetryReady { .. } => "node_retry_ready",
        GraphEvent::RunCancelled { .. } => "run_cancelled",
        GraphEvent::RunRecovered { .. } => "graph_recovered",
        GraphEvent::LoopStateUpdated { .. } => "loop_state_updated",
        GraphEvent::CompensationUpdated { .. } => "compensation_updated",
    }
}

fn reconcile_budget_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    node_id: &str,
    attempt: u32,
    actual: Option<UsageAccounting>,
    at_ms: i64,
) -> Result<(), StoreError> {
    let reservation_exists = tx
        .query_row(
            "SELECT 1 FROM budget_reservations
             WHERE run_id = ?1 AND node_id = ?2 AND attempt = ?3",
            params![run_id, node_id, attempt],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !reservation_exists {
        return Ok(());
    }
    let spec_raw: String = tx.query_row(
        "SELECT graph_specs.spec_json FROM graph_runs
         JOIN graph_specs ON graph_specs.hash = graph_runs.spec_hash
         WHERE graph_runs.id = ?1",
        params![run_id],
        |row| row.get(0),
    )?;
    let spec: GraphSpec = serde_json::from_str(&spec_raw)?;
    let state_raw: String = tx.query_row(
        "SELECT state_json FROM run_budget_state WHERE run_id = ?1",
        params![run_id],
        |row| row.get(0),
    )?;
    let mut ledger =
        BudgetLedger::from_persistent_state(spec.spec.budgets, serde_json::from_str(&state_raw)?);
    let reservation_id = BudgetReservationId {
        run_id: run_id.to_string(),
        node_id: node_id.to_string(),
        attempt,
    };
    let charged = ledger.reconcile(&reservation_id, actual)?;
    tx.execute(
        "UPDATE run_budget_state SET state_json = ?1, updated_at_ms = ?2 WHERE run_id = ?3",
        params![
            serde_json::to_string(&ledger.persistent_state())?,
            at_ms,
            run_id
        ],
    )?;
    tx.execute(
        "DELETE FROM budget_reservations WHERE run_id = ?1 AND node_id = ?2 AND attempt = ?3",
        params![run_id, node_id, attempt],
    )?;
    tx.execute(
        "INSERT INTO usage_records_v2(run_id, node_id, attempt, usage_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(run_id, node_id, attempt)
         DO UPDATE SET usage_json = excluded.usage_json",
        params![
            run_id,
            node_id,
            attempt,
            serde_json::to_string(&charged)?,
            at_ms
        ],
    )?;
    Ok(())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn status_text<T: Serialize>(status: T) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_run_status(raw: String) -> rusqlite::Result<RunStatus> {
    serde_json::from_str(&format!("{raw:?}")).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_node_status(raw: String) -> rusqlite::Result<NodeStatus> {
    serde_json::from_str(&format!("{raw:?}")).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_json_enum<T: serde::de::DeserializeOwned>(
    raw: String,
    column: usize,
) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("{raw:?}")).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })
}
