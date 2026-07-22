use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use xai_sqlite_journal::JournalMode;

use super::approval::ExecutionApproval;
use super::normalization::canonical_graph_hash;
use super::types::{GraphSpec, NodeId, NodeOutput, NodeStatus, RunStatus, UsageAccounting};

pub const STORE_SCHEMA_VERSION: u32 = 1;

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
    RunCancelled {
        reason: String,
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
            NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Retrying | NodeStatus::Stale
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
        }
        GraphEvent::RunCancelled { .. } => {
            tx.execute(
                "UPDATE graph_runs SET status = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![status_text(RunStatus::Cancelled), now, run_id],
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
        GraphEvent::RunCancelled { .. } => "run_cancelled",
    }
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
