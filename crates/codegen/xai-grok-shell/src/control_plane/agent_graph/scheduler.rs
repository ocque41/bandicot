use std::collections::{BTreeMap, BTreeSet};

use futures::stream::{FuturesUnordered, StreamExt as _};
use thiserror::Error;

use super::budget::{BudgetError, BudgetLedger, BudgetReservationId, BudgetSnapshot};
use super::resources::{ResourceError, ResourceManager, ResourceTicket};
use super::store::{AgentGraphStore, StoreError};
use super::types::{
    AGENTGRAPH_SCHEMA_VERSION, EdgeSpec, GraphSpec, JoinPolicy, NodeId, NodeKind, NodeOutput,
    NodeSpec, NodeStatus, RunStatus, UsageAccounting,
};
use super::validation::{ValidationOptions, validate_graph_spec};
use super::worker::{
    SubagentWorkerBackend, WorkerBackend, WorkerCompletion, WorkerError, WorkerRequest,
    graph_worker_id,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub max_active_model_calls: u32,
    pub plan_mode: bool,
}

impl SchedulerConfig {
    pub fn from_graph(spec: &GraphSpec) -> Self {
        Self {
            max_active_model_calls: spec.spec.execution.max_active_model_calls.max(1),
            plan_mode: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerReport {
    pub run_status: RunStatus,
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub failed_nodes: usize,
    pub timed_out_nodes: usize,
    pub cancelled_nodes: usize,
    pub peak_active_workers: usize,
    pub queued_workers: usize,
    pub budget: BudgetSnapshot,
}

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("graph did not pass validation: {0} error(s)")]
    InvalidGraph(usize),
    #[error("worker error: {0}")]
    Worker(#[from] WorkerError),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("budget error: {0}")]
    Budget(#[from] BudgetError),
    #[error("resource error on node `{node_id}`: {source}")]
    Resource {
        node_id: NodeId,
        #[source]
        source: ResourceError,
    },
}

pub struct AgentGraphScheduler {
    spec: GraphSpec,
    config: SchedulerConfig,
    resources: ResourceManager,
    dependencies: BTreeMap<NodeId, BTreeSet<NodeId>>,
    incoming_edges: BTreeMap<NodeId, Vec<EdgeSpec>>,
    nodes: BTreeMap<NodeId, NodeSpec>,
    statuses: BTreeMap<NodeId, NodeStatus>,
    attempts: BTreeMap<NodeId, u32>,
    active_tickets: BTreeMap<NodeId, ResourceTicket>,
    active_workers: BTreeMap<String, NodeId>,
    run_status: RunStatus,
    peak_active_workers: usize,
    queued_workers: BTreeSet<NodeId>,
    budget: BudgetLedger,
    budget_reservations: BTreeMap<NodeId, BudgetReservationId>,
}

impl AgentGraphScheduler {
    pub fn new(spec: GraphSpec, config: SchedulerConfig) -> Result<Self, SchedulerError> {
        let report = validate_graph_spec(&spec, &ValidationOptions::default());
        if !report.is_valid() {
            return Err(SchedulerError::InvalidGraph(report.errors.len()));
        }
        let nodes = spec
            .spec
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<BTreeMap<_, _>>();
        let dependencies = build_dependency_map(&spec);
        let incoming_edges = build_incoming_edges(&spec);
        let statuses = nodes
            .keys()
            .map(|node_id| (node_id.clone(), NodeStatus::Pending))
            .collect();
        let budget = BudgetLedger::new(spec.spec.budgets.clone());
        Ok(Self {
            resources: ResourceManager::from_graph(&spec),
            spec,
            config,
            dependencies,
            incoming_edges,
            nodes,
            statuses,
            attempts: BTreeMap::new(),
            active_tickets: BTreeMap::new(),
            active_workers: BTreeMap::new(),
            run_status: RunStatus::AwaitingApproval,
            peak_active_workers: 0,
            queued_workers: BTreeSet::new(),
            budget,
            budget_reservations: BTreeMap::new(),
        })
    }

    pub fn start(&mut self) {
        if matches!(
            self.run_status,
            RunStatus::AwaitingApproval | RunStatus::Validated
        ) {
            self.run_status = RunStatus::Running;
        }
    }

    pub fn pause(&mut self) {
        if self.run_status == RunStatus::Running {
            self.run_status = RunStatus::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.run_status == RunStatus::Paused {
            self.run_status = RunStatus::Running;
        }
    }

    pub fn drain(&mut self) {
        if self.run_status == RunStatus::Running {
            self.run_status = RunStatus::Draining;
        }
    }

    pub fn cancel<B: WorkerBackend>(&mut self, backend: &mut B) {
        for worker_id in self.active_workers.keys().cloned().collect::<Vec<_>>() {
            let _ = backend.cancel(&worker_id);
        }
        for ticket in self.active_tickets.values().cloned().collect::<Vec<_>>() {
            self.resources.release(ticket);
        }
        self.active_workers.clear();
        self.active_tickets.clear();
        for reservation in self
            .budget_reservations
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.budget.release(&reservation);
        }
        self.budget_reservations.clear();
        for status in self.statuses.values_mut() {
            if !status.is_terminal() {
                *status = NodeStatus::Cancelled;
            }
        }
        self.run_status = RunStatus::Cancelled;
    }

    pub fn ready_nodes(&self) -> Vec<NodeId> {
        if self.run_status != RunStatus::Running {
            return Vec::new();
        }
        self.nodes
            .keys()
            .filter(|node_id| {
                self.statuses.get(*node_id).is_some_and(|status| {
                    matches!(
                        status,
                        NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Retrying
                    )
                }) && self.dependencies_satisfied(node_id)
            })
            .cloned()
            .collect()
    }

    pub fn step<B: WorkerBackend>(
        &mut self,
        backend: &mut B,
        run_id: &str,
    ) -> Result<(), SchedulerError> {
        if self.run_status != RunStatus::Running {
            return Ok(());
        }

        let ready = self.ready_nodes();
        for node_id in ready {
            let node = self.nodes.get(&node_id).expect("ready node exists").clone();
            let active_model_workers = self.active_model_worker_count();
            if node.is_model_worker()
                && active_model_workers >= self.config.max_active_model_calls as usize
            {
                self.queued_workers.insert(node_id);
                continue;
            }

            let next_attempt = self.attempts.get(&node_id).copied().unwrap_or(0) + 1;
            if !self.reserve_node_budget(run_id, &node, next_attempt)? {
                break;
            }
            let ticket = match self.resources.try_acquire(
                &node,
                &self.spec.spec.defaults,
                self.config.plan_mode,
            ) {
                Ok(ticket) => ticket,
                Err(source) if source.is_contention() => {
                    self.release_node_budget(&node_id);
                    self.queued_workers.insert(node_id);
                    continue;
                }
                Err(source) => {
                    self.release_node_budget(&node_id);
                    return Err(SchedulerError::Resource {
                        node_id: node_id.clone(),
                        source,
                    });
                }
            };
            self.active_tickets.insert(node_id.clone(), ticket);
            *self.statuses.get_mut(&node_id).expect("node status exists") = NodeStatus::Running;
            let attempt = self.attempts.entry(node_id.clone()).or_default();
            *attempt += 1;

            if node.is_model_worker() {
                let handle = backend.start(WorkerRequest {
                    graph_run_id: run_id.to_string(),
                    node_id: node_id.clone(),
                    attempt: *attempt,
                    objective: node.objective.clone(),
                })?;
                self.active_workers
                    .insert(handle.worker_id.clone(), node_id.clone());
                self.peak_active_workers = self
                    .peak_active_workers
                    .max(self.active_model_worker_count());
            } else {
                self.complete_node(&node_id, NodeStatus::Succeeded);
            }
        }

        for completion in backend.poll_completed() {
            self.accept_completion(completion);
        }
        self.refresh_run_status();
        Ok(())
    }

    pub fn run_to_completion<B: WorkerBackend>(
        &mut self,
        backend: &mut B,
        run_id: &str,
    ) -> Result<SchedulerReport, SchedulerError> {
        self.start();
        let max_steps = self.nodes.len().saturating_mul(4).max(1);
        for _ in 0..max_steps {
            let before = self.completed_nodes();
            self.step(backend, run_id)?;
            if matches!(
                self.run_status,
                RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
            ) {
                break;
            }
            if self.completed_nodes() == before && self.ready_nodes().is_empty() {
                self.refresh_run_status();
                break;
            }
        }
        Ok(self.report())
    }

    pub async fn run_to_completion_with_subagents(
        &mut self,
        backend: &SubagentWorkerBackend,
        store: &mut AgentGraphStore,
        run_id: &str,
    ) -> Result<SchedulerReport, SchedulerError> {
        self.start();
        if store.run_status(run_id)? == RunStatus::Cancelled {
            self.run_status = RunStatus::Cancelled;
            return Ok(self.report());
        }
        store.mark_run_status(run_id, RunStatus::Running)?;
        let max_steps = self.nodes.len().saturating_mul(4).max(1);
        for _ in 0..max_steps {
            if !self.wait_for_runnable_status(store, run_id).await? {
                break;
            }
            let before = self.terminal_nodes();
            self.step_with_subagents(backend, store, run_id).await?;
            if matches!(
                self.run_status,
                RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
            ) {
                break;
            }
            if self.terminal_nodes() == before && self.ready_nodes().is_empty() {
                self.refresh_run_status();
                break;
            }
        }
        let report = self.report();
        store.mark_run_status(run_id, report.run_status)?;
        Ok(report)
    }

    pub fn node_status(&self, node_id: &str) -> Option<NodeStatus> {
        self.statuses.get(node_id).copied()
    }

    pub fn report(&self) -> SchedulerReport {
        SchedulerReport {
            run_status: self.run_status,
            total_nodes: self.nodes.len(),
            completed_nodes: self.count_status(NodeStatus::Succeeded),
            failed_nodes: self.count_status(NodeStatus::Failed),
            timed_out_nodes: self.count_status(NodeStatus::TimedOut),
            cancelled_nodes: self.count_status(NodeStatus::Cancelled),
            peak_active_workers: self.peak_active_workers,
            queued_workers: self.queued_workers.len(),
            budget: self.budget.snapshot(),
        }
    }

    fn accept_completion(&mut self, completion: WorkerCompletion) {
        if let Some(worker_id) = self.active_workers.iter().find_map(|(worker_id, node_id)| {
            (node_id == &completion.node_id).then_some(worker_id.clone())
        }) {
            self.active_workers.remove(&worker_id);
        }
        self.complete_node_with_usage(
            &completion.node_id,
            completion.output.status,
            Some(completion.usage),
        );
    }

    fn complete_node(&mut self, node_id: &str, status: NodeStatus) {
        self.complete_node_with_usage(node_id, status, None);
    }

    fn complete_node_with_usage(
        &mut self,
        node_id: &str,
        status: NodeStatus,
        usage: Option<UsageAccounting>,
    ) {
        if let Some(ticket) = self.active_tickets.remove(node_id) {
            self.resources.release(ticket);
        }
        if let Some(reservation) = self.budget_reservations.remove(node_id) {
            let _ = self.budget.reconcile(&reservation, usage);
        }
        if let Some(slot) = self.statuses.get_mut(node_id) {
            *slot = status;
        }
    }

    fn active_model_worker_count(&self) -> usize {
        self.active_workers.len()
    }

    fn completed_nodes(&self) -> usize {
        self.count_status(NodeStatus::Succeeded)
    }

    fn terminal_nodes(&self) -> usize {
        self.statuses
            .values()
            .filter(|status| (**status).is_terminal())
            .count()
    }

    fn terminal_error_nodes(&self) -> usize {
        self.statuses
            .values()
            .filter(|status| {
                matches!(
                    **status,
                    NodeStatus::Failed | NodeStatus::TimedOut | NodeStatus::Cancelled
                )
            })
            .count()
    }

    fn count_status(&self, needle: NodeStatus) -> usize {
        self.statuses
            .values()
            .filter(|status| **status == needle)
            .count()
    }

    fn refresh_run_status(&mut self) {
        if self.run_status == RunStatus::Cancelled {
            return;
        }
        if self.terminal_error_nodes() > 0 {
            self.run_status = RunStatus::Failed;
        } else if self
            .statuses
            .values()
            .all(|status| *status == NodeStatus::Succeeded)
        {
            self.run_status = RunStatus::Completed;
        }
    }

    async fn step_with_subagents(
        &mut self,
        backend: &SubagentWorkerBackend,
        store: &mut AgentGraphStore,
        run_id: &str,
    ) -> Result<(), SchedulerError> {
        if self.run_status != RunStatus::Running {
            return Ok(());
        }

        let ready = self.ready_nodes();
        let model_limit = self.config.max_active_model_calls.max(1) as usize;
        let mut selected_model_workers = 0usize;
        let mut pending_model_nodes = Vec::new();
        for node_id in ready {
            let node = self.nodes.get(&node_id).expect("ready node exists").clone();
            if node.is_model_worker() && selected_model_workers >= model_limit {
                self.queued_workers.insert(node_id);
                continue;
            }

            let next_attempt = self.attempts.get(&node_id).copied().unwrap_or(0) + 1;
            if !self.reserve_node_budget(run_id, &node, next_attempt)? {
                store.mark_run_status(run_id, RunStatus::BudgetStopped)?;
                break;
            }
            let ticket = match self.resources.try_acquire(
                &node,
                &self.spec.spec.defaults,
                self.config.plan_mode,
            ) {
                Ok(ticket) => ticket,
                Err(source) if source.is_contention() => {
                    self.release_node_budget(&node_id);
                    self.queued_workers.insert(node_id);
                    continue;
                }
                Err(source) => {
                    self.release_node_budget(&node_id);
                    return Err(SchedulerError::Resource {
                        node_id: node_id.clone(),
                        source,
                    });
                }
            };
            self.active_tickets.insert(node_id.clone(), ticket);
            *self.statuses.get_mut(&node_id).expect("node status exists") = NodeStatus::Running;

            if node.is_model_worker() {
                let worker_id = graph_worker_id(run_id, &node_id, next_attempt);
                let Some(lease) =
                    store.lease_node(run_id, &node_id, &worker_id, default_lease_ttl_ms())?
                else {
                    self.release_node_budget(&node_id);
                    self.queued_workers.insert(node_id.clone());
                    self.complete_node(&node_id, NodeStatus::Pending);
                    continue;
                };
                self.attempts.insert(node_id.clone(), lease.attempt);
                selected_model_workers += 1;
                pending_model_nodes.push((node_id, node, lease.attempt, lease.owner));
            } else {
                store.mark_node_status(run_id, &node_id, NodeStatus::Succeeded)?;
                self.complete_node(&node_id, NodeStatus::Succeeded);
            }
        }

        if !pending_model_nodes.is_empty() {
            self.peak_active_workers = self.peak_active_workers.max(pending_model_nodes.len());
            let defaults = self.spec.spec.defaults.clone();
            let schemas = self.spec.spec.schemas.clone();
            let mut futures = FuturesUnordered::new();
            for (node_id, node, attempt, worker_id) in pending_model_nodes {
                let backend = backend.clone();
                let run_id = run_id.to_string();
                let defaults = defaults.clone();
                let schemas = schemas.clone();
                futures.push(async move {
                    let result = backend
                        .run_node(&run_id, &node, &defaults, &schemas, attempt, worker_id)
                        .await;
                    (node_id, attempt, result)
                });
            }

            let mut completed = Vec::new();
            while let Some(result) = futures.next().await {
                completed.push(result);
            }
            completed.sort_by(|left, right| left.0.cmp(&right.0));

            for (node_id, attempt, result) in completed {
                let completion = match result {
                    Ok(completion) => completion,
                    Err(err) => worker_error_completion(run_id, &node_id, attempt, err),
                };
                let accepted = store.accept_output(
                    run_id,
                    &node_id,
                    completion.attempt,
                    &completion.output,
                )?;
                if !accepted {
                    return Err(SchedulerError::Worker(WorkerError::Rejected {
                        node_id,
                        reason: "store rejected stale node output attempt".to_string(),
                    }));
                }
                store.record_usage(
                    run_id,
                    &completion.node_id,
                    completion.attempt,
                    &completion.usage,
                )?;
                self.complete_node_with_usage(
                    &completion.node_id,
                    completion.output.status,
                    Some(completion.usage),
                );
            }
        }

        self.refresh_run_status();
        Ok(())
    }

    async fn wait_for_runnable_status(
        &mut self,
        store: &mut AgentGraphStore,
        run_id: &str,
    ) -> Result<bool, SchedulerError> {
        loop {
            match store.run_status(run_id)? {
                RunStatus::Cancelled => {
                    self.run_status = RunStatus::Cancelled;
                    return Ok(false);
                }
                RunStatus::Paused | RunStatus::Pausing => {
                    self.run_status = RunStatus::Paused;
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                RunStatus::Draining => {
                    self.run_status = RunStatus::Draining;
                    return Ok(false);
                }
                RunStatus::Completed
                | RunStatus::PartiallyCompleted
                | RunStatus::Failed
                | RunStatus::BudgetStopped
                | RunStatus::Compensated
                | RunStatus::CompensationFailed
                | RunStatus::ManualInterventionRequired => {
                    return Ok(false);
                }
                RunStatus::Draft
                | RunStatus::Validating
                | RunStatus::Validated
                | RunStatus::AwaitingApproval
                | RunStatus::Running
                | RunStatus::Reducing
                | RunStatus::Verifying
                | RunStatus::Compensating => {
                    self.run_status = RunStatus::Running;
                    return Ok(true);
                }
            }
        }
    }

    fn dependencies_satisfied(&self, node_id: &str) -> bool {
        let Some(deps) = self.dependencies.get(node_id) else {
            return true;
        };
        if deps.is_empty() {
            return true;
        }

        let incoming = self
            .incoming_edges
            .get(node_id)
            .cloned()
            .unwrap_or_default();
        if let Some(join) = strongest_join(&incoming) {
            let successes = deps
                .iter()
                .filter(|dep| self.statuses.get(*dep) == Some(&NodeStatus::Succeeded))
                .count() as u32;
            return match join.policy {
                JoinPolicy::AnySuccess | JoinPolicy::FirstValid => successes >= 1,
                JoinPolicy::MinimumSuccess | JoinPolicy::Quorum => {
                    successes >= join.required.unwrap_or(deps.len() as u32)
                }
                JoinPolicy::DeadlineBestEffort => deps.iter().any(|dep| {
                    self.statuses
                        .get(dep)
                        .is_some_and(|status| status.is_terminal())
                }),
                JoinPolicy::AllSuccess => successes == deps.len() as u32,
                JoinPolicy::AllTerminal => deps.iter().all(|dep| {
                    self.statuses
                        .get(dep)
                        .is_some_and(|status| status.is_terminal())
                }),
            };
        }

        deps.iter()
            .all(|dep| self.statuses.get(dep) == Some(&NodeStatus::Succeeded))
    }

    fn reserve_node_budget(
        &mut self,
        run_id: &str,
        node: &NodeSpec,
        attempt: u32,
    ) -> Result<bool, SchedulerError> {
        let id = BudgetReservationId {
            run_id: run_id.to_string(),
            node_id: node.id.clone(),
            attempt,
        };
        let estimate = BudgetLedger::estimate_node(node, &self.spec.spec.defaults);
        match self.budget.reserve(id.clone(), estimate) {
            Ok(()) => {
                self.budget_reservations.insert(node.id.clone(), id);
                Ok(true)
            }
            Err(BudgetError::Exceeded { .. } | BudgetError::Stopped(_)) => {
                self.budget.stop("hard run budget reached");
                self.run_status = RunStatus::BudgetStopped;
                Ok(false)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn release_node_budget(&mut self, node_id: &str) {
        if let Some(id) = self.budget_reservations.remove(node_id) {
            self.budget.release(&id);
        }
    }
}

fn strongest_join(edges: &[EdgeSpec]) -> Option<super::types::JoinSpec> {
    edges.iter().filter_map(|edge| edge.join.clone()).next()
}

fn build_dependency_map(spec: &GraphSpec) -> BTreeMap<NodeId, BTreeSet<NodeId>> {
    let mut dependencies = spec
        .spec
        .nodes
        .iter()
        .map(|node| (node.id.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in &spec.spec.edges {
        dependencies
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }
    for node in &spec.spec.nodes {
        for binding in node.inputs.values() {
            dependencies
                .entry(node.id.clone())
                .or_default()
                .insert(binding.from_node.clone());
        }
        if let Some(map) = &node.map {
            dependencies
                .entry(node.id.clone())
                .or_default()
                .insert(map.from_node.clone());
        }
        if let Some(reduce) = &node.reduce {
            dependencies
                .entry(node.id.clone())
                .or_default()
                .insert(reduce.from_node.clone());
        }
        for route in &node.routes {
            dependencies
                .entry(route.to.clone())
                .or_default()
                .insert(node.id.clone());
        }
    }
    dependencies
}

fn build_incoming_edges(spec: &GraphSpec) -> BTreeMap<NodeId, Vec<EdgeSpec>> {
    let mut incoming: BTreeMap<NodeId, Vec<EdgeSpec>> = BTreeMap::new();
    for edge in &spec.spec.edges {
        incoming
            .entry(edge.to.clone())
            .or_default()
            .push(edge.clone());
    }
    incoming
}

pub fn is_model_worker_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Agent
            | NodeKind::MapAgent
            | NodeKind::ReduceAgent
            | NodeKind::Verifier
            | NodeKind::Router
    )
}

fn default_lease_ttl_ms() -> i64 {
    24 * 60 * 60 * 1000
}

fn worker_error_completion(
    graph_run_id: &str,
    node_id: &str,
    attempt: u32,
    err: WorkerError,
) -> WorkerCompletion {
    WorkerCompletion {
        graph_run_id: graph_run_id.to_string(),
        node_id: node_id.to_string(),
        attempt,
        output: NodeOutput {
            schema_version: AGENTGRAPH_SCHEMA_VERSION,
            graph_run_id: graph_run_id.to_string(),
            node_instance_id: node_id.to_string(),
            attempt_id: format!("attempt-{attempt}"),
            status: NodeStatus::Failed,
            summary: format!("worker failed before producing output: {err}"),
            findings: Vec::new(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            tests_run: Vec::new(),
            artifacts: Vec::new(),
            assumptions: Vec::new(),
            blockers: vec![err.to_string()],
            confidence: 0.0,
        },
        usage: UsageAccounting {
            model_calls: 1,
            node_attempts: 1,
            failures: 1,
            ..UsageAccounting::default()
        },
    }
}
