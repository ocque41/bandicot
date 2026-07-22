use std::collections::{BTreeMap, BTreeSet};

use futures::stream::{FuturesUnordered, StreamExt as _};
use thiserror::Error;

use super::admission::{
    AdmissionClass, AdmissionRequest, AdmissionRequestId, AdmissionResult, AdmissionTicket,
    global_admission_controller,
};
use super::budget::{BudgetError, BudgetLedger, BudgetReservationId, BudgetSnapshot};
use super::compensation::{
    CompensationPlan, CompensationPlanStatus, build_compensation_plan,
    completed_node_order_from_events,
};
use super::loop_controller::{LoopDecision, LoopState, advance_loop};
use super::rate_limit::{
    ProviderAdmission, ProviderCapacityConfig, ProviderCapacityController, ProviderPermit,
    ProviderRouteKey,
};
use super::resources::{ResourceError, ResourceManager, ResourceTicket};
use super::retry::{RetryDecision, classify_node_output, decide_retry};
use super::store::{
    AgentGraphStore, GraphRunSnapshot, LeaseReservationOutcome, StoreError, now_ms,
};
use super::types::{
    AGENTGRAPH_SCHEMA_VERSION, EdgeSpec, GraphSpec, JoinPolicy, NodeId, NodeKind, NodeOutput,
    NodeSpec, NodeStatus, OrchestrationMode, RunStatus, UsageAccounting,
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
    #[error("host admission rejected node `{node_id}`: {reason}")]
    Admission { node_id: NodeId, reason: String },
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
    partial_completion: bool,
    completion_order: Vec<NodeId>,
    compensation_plan: Option<CompensationPlan>,
    host_tickets: BTreeMap<NodeId, AdmissionTicket>,
    host_requests: BTreeMap<NodeId, AdmissionRequestId>,
    swarm_ramp_limit: usize,
    provider_capacity: ProviderCapacityController,
    provider_permits: BTreeMap<NodeId, ProviderPermit>,
    provider_routes: BTreeMap<NodeId, ProviderRouteKey>,
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
            .iter()
            .map(|(node_id, node)| {
                (
                    node_id.clone(),
                    if node.kind == NodeKind::Compensation {
                        NodeStatus::Blocked
                    } else {
                        NodeStatus::Pending
                    },
                )
            })
            .collect();
        let budget = BudgetLedger::new(spec.spec.budgets.clone());
        let configured_limit = config.max_active_model_calls.max(1) as usize;
        let swarm_ramp_limit =
            if spec.spec.execution.orchestration_policy == OrchestrationMode::Swarm {
                configured_limit.min(4)
            } else {
                configured_limit
            };
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
            partial_completion: false,
            completion_order: Vec::new(),
            compensation_plan: None,
            host_tickets: BTreeMap::new(),
            host_requests: BTreeMap::new(),
            swarm_ramp_limit,
            provider_capacity: ProviderCapacityController::default(),
            provider_permits: BTreeMap::new(),
            provider_routes: BTreeMap::new(),
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

    pub fn restore_from_snapshot(&mut self, snapshot: &GraphRunSnapshot) {
        self.run_status = snapshot.status;
        self.completion_order = completed_node_order_from_events(&snapshot.events);
        for node in &snapshot.node_states {
            if self.statuses.contains_key(&node.node_id) {
                self.statuses.insert(node.node_id.clone(), node.status);
                self.attempts.insert(node.node_id.clone(), node.attempt);
            }
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
        self.release_all_provider_admission();
        self.release_all_host_admission();
        for status in self.statuses.values_mut() {
            if !status.is_terminal() {
                *status = NodeStatus::Cancelled;
            }
        }
        self.run_status = RunStatus::Cancelled;
    }

    pub fn ready_nodes(&self) -> Vec<NodeId> {
        if self.run_status == RunStatus::Compensating {
            return self
                .compensation_plan
                .as_ref()
                .and_then(CompensationPlan::current)
                .map(|step| step.compensation_node_id.clone())
                .into_iter()
                .filter(|node_id| {
                    self.statuses.get(node_id).is_some_and(|status| {
                        matches!(
                            status,
                            NodeStatus::Pending
                                | NodeStatus::Blocked
                                | NodeStatus::Ready
                                | NodeStatus::Retrying
                                | NodeStatus::Stale
                        )
                    })
                })
                .collect();
        }
        if self.run_status != RunStatus::Running {
            return Vec::new();
        }
        self.nodes
            .keys()
            .filter(|node_id| {
                self.nodes
                    .get(*node_id)
                    .is_some_and(|node| node.kind != NodeKind::Compensation)
                    && self.statuses.get(*node_id).is_some_and(|status| {
                        matches!(
                            status,
                            NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Retrying
                        )
                    })
                    && self.dependencies_satisfied(node_id)
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
        let max_steps = self.maximum_scheduler_steps();
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
        let persisted_status = store.run_status(run_id)?;
        if persisted_status == RunStatus::Cancelled {
            self.run_status = RunStatus::Cancelled;
            return Ok(self.report());
        }
        if persisted_status == RunStatus::Compensating {
            let snapshot = store.replay_run(run_id)?;
            self.restore_from_snapshot(&snapshot);
            self.compensation_plan = store.compensation_plan(run_id)?;
        } else {
            self.start();
            store.mark_run_status(run_id, RunStatus::Running)?;
        }
        let max_steps = self.maximum_scheduler_steps();
        for _ in 0..max_steps {
            for schedule in store.activate_due_retries_for_run(run_id, now_ms())? {
                self.statuses
                    .insert(schedule.node_id.clone(), NodeStatus::Ready);
            }
            if !self.wait_for_runnable_status(store, run_id).await? {
                break;
            }
            let before = self.terminal_nodes();
            self.step_with_subagents(backend, store, run_id).await?;
            if run_is_terminal(self.run_status) {
                break;
            }
            if self.terminal_nodes() == before && self.ready_nodes().is_empty() {
                if let Some(deadline) = store.next_retry_at(run_id)? {
                    let delay_ms = deadline.saturating_sub(now_ms()).max(1) as u64;
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms.min(1_000))).await;
                    continue;
                }
                self.refresh_run_status();
                break;
            }
        }
        let report = self.report();
        self.release_all_host_admission();
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
        self.release_host_admission(node_id);
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
        if matches!(
            self.run_status,
            RunStatus::Cancelled
                | RunStatus::Compensating
                | RunStatus::Compensated
                | RunStatus::CompensationFailed
                | RunStatus::ManualInterventionRequired
        ) {
            return;
        }
        if self.partial_completion {
            self.run_status = RunStatus::PartiallyCompleted;
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
        if !matches!(
            self.run_status,
            RunStatus::Running | RunStatus::Compensating
        ) {
            return Ok(());
        }

        let ready = self.ready_nodes();
        let model_limit =
            (self.config.max_active_model_calls.max(1) as usize).min(self.swarm_ramp_limit.max(1));
        let mut selected_model_workers = 0usize;
        let mut pending_model_nodes = Vec::new();
        for node_id in ready {
            let node = self.nodes.get(&node_id).expect("ready node exists").clone();
            if node.is_model_worker() && selected_model_workers >= model_limit {
                self.queued_workers.insert(node_id);
                continue;
            }

            let next_attempt = self.attempts.get(&node_id).copied().unwrap_or(0) + 1;
            let estimate = BudgetLedger::estimate_node(&node, &self.spec.spec.defaults);
            if !self.acquire_host_admission(run_id, &node, next_attempt)? {
                self.queued_workers.insert(node_id);
                continue;
            }
            if node.is_model_worker()
                && !self.acquire_provider_admission(backend, &node, &estimate)?
            {
                self.release_host_admission(&node_id);
                self.queued_workers.insert(node_id);
                continue;
            }

            let ticket = match self.resources.try_acquire(
                &node,
                &self.spec.spec.defaults,
                self.config.plan_mode,
            ) {
                Ok(ticket) => ticket,
                Err(source) if source.is_contention() => {
                    self.release_provider_admission(&node_id, None);
                    self.release_host_admission(&node_id);
                    self.queued_workers.insert(node_id);
                    continue;
                }
                Err(source) => {
                    self.release_provider_admission(&node_id, None);
                    self.release_host_admission(&node_id);
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
                let lease = match store.reserve_and_lease_node(
                    run_id,
                    &node_id,
                    &worker_id,
                    default_lease_ttl_ms(),
                    estimate.clone(),
                )? {
                    LeaseReservationOutcome::Leased(lease) => lease,
                    LeaseReservationOutcome::NotRunnable => {
                        if let Some(ticket) = self.active_tickets.remove(&node_id) {
                            self.resources.release(ticket);
                        }
                        self.release_provider_admission(&node_id, None);
                        self.release_host_admission(&node_id);
                        self.queued_workers.insert(node_id.clone());
                        self.statuses.insert(node_id, NodeStatus::Pending);
                        continue;
                    }
                    LeaseReservationOutcome::BudgetStopped { .. } => {
                        if let Some(ticket) = self.active_tickets.remove(&node_id) {
                            self.resources.release(ticket);
                        }
                        self.release_provider_admission(&node_id, None);
                        self.release_host_admission(&node_id);
                        self.budget.stop("hard durable run budget reached");
                        self.run_status = RunStatus::BudgetStopped;
                        break;
                    }
                };
                let reservation_id = BudgetReservationId {
                    run_id: run_id.to_string(),
                    node_id: node_id.clone(),
                    attempt: lease.attempt,
                };
                self.budget.reserve(reservation_id.clone(), estimate)?;
                self.budget_reservations
                    .insert(node_id.clone(), reservation_id);
                self.attempts.insert(node_id.clone(), lease.attempt);
                if node.kind == NodeKind::Compensation
                    && let Some(plan) = self.compensation_plan.as_mut()
                {
                    plan.mark_running(lease.attempt);
                    store.save_compensation_plan(run_id, plan)?;
                    self.statuses
                        .insert(node_id.clone(), NodeStatus::Compensating);
                }
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

            let mut batch_succeeded = true;
            let mut batch_rate_limited = false;
            for (node_id, attempt, result) in completed {
                let completion = match result {
                    Ok(completion) => completion,
                    Err(err) => worker_error_completion(run_id, &node_id, attempt, err),
                };
                let classification = classify_node_output(&completion.output);
                batch_succeeded &= completion.output.status == NodeStatus::Succeeded;
                batch_rate_limited |=
                    classification == super::retry::FailureClassification::RateLimited;
                let actual_tokens = completion
                    .usage
                    .input_tokens
                    .saturating_add(completion.usage.output_tokens);
                let mut observed_route =
                    self.release_provider_admission(&node_id, Some(actual_tokens));
                if let Some(observation) = &completion.provider_capacity {
                    if !self.provider_capacity.contains_route(&observation.route) {
                        self.provider_capacity.configure(
                            observation.route.clone(),
                            ProviderCapacityConfig::default(),
                        );
                    }
                    self.provider_capacity.observe_headers(
                        &observation.route,
                        &observation.rate_limits,
                        now_ms(),
                    );
                    observed_route = Some(observation.route.clone());
                }
                if let Some(route) = observed_route.as_ref() {
                    match classification {
                        super::retry::FailureClassification::RateLimited => {
                            let retry_after_ms = completion
                                .provider_capacity
                                .as_ref()
                                .and_then(|observation| observation.rate_limits.retry_after_ms);
                            self.provider_capacity.observe_rate_limited(
                                route,
                                retry_after_ms,
                                now_ms(),
                            );
                        }
                        super::retry::FailureClassification::Unauthorized
                        | super::retry::FailureClassification::Forbidden => {
                            self.provider_capacity.observe_auth_failure(route);
                        }
                        _ => {}
                    }
                }
                let accepted = store.accept_output_and_reconcile(
                    run_id,
                    &node_id,
                    completion.attempt,
                    &completion.output,
                    Some(&completion.usage),
                )?;
                if !accepted {
                    return Err(SchedulerError::Worker(WorkerError::Rejected {
                        node_id,
                        reason: "store rejected stale node output attempt".to_string(),
                    }));
                }
                self.complete_node_with_usage(
                    &completion.node_id,
                    completion.output.status,
                    Some(completion.usage.clone()),
                );
                let is_compensation = self
                    .nodes
                    .get(&completion.node_id)
                    .is_some_and(|node| node.kind == NodeKind::Compensation);
                if completion.output.status == NodeStatus::Succeeded && !is_compensation {
                    if !self.completion_order.contains(&completion.node_id) {
                        self.completion_order.push(completion.node_id.clone());
                    }
                }
                if completion.output.status == NodeStatus::Succeeded
                    && self
                        .nodes
                        .get(&completion.node_id)
                        .is_some_and(|node| node.kind == NodeKind::Loop)
                {
                    self.advance_loop_node(store, run_id, &completion)?;
                }
                let retry_scheduled =
                    matches!(
                        completion.output.status,
                        NodeStatus::Failed | NodeStatus::TimedOut
                    ) && self.schedule_retry_for_completion(store, run_id, &completion)?;
                if retry_scheduled {
                    self.statuses
                        .insert(completion.node_id.clone(), NodeStatus::Retrying);
                }
                if is_compensation && !retry_scheduled {
                    self.finish_compensation_step(store, run_id, &completion)?;
                }
            }
            if self.spec.spec.execution.orchestration_policy == OrchestrationMode::Swarm {
                let ceiling = self.config.max_active_model_calls.max(1) as usize;
                if batch_rate_limited {
                    self.swarm_ramp_limit = (self.swarm_ramp_limit / 2).max(1);
                } else if batch_succeeded {
                    self.swarm_ramp_limit = self.swarm_ramp_limit.saturating_mul(2).min(ceiling);
                }
            }
        }

        self.refresh_run_status();
        if self.run_status == RunStatus::Failed {
            self.maybe_start_compensation(store, run_id)?;
        }
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
                | RunStatus::Verifying => {
                    self.run_status = RunStatus::Running;
                    return Ok(true);
                }
                RunStatus::Compensating => {
                    self.run_status = RunStatus::Compensating;
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

    fn schedule_retry_for_completion(
        &self,
        store: &mut AgentGraphStore,
        run_id: &str,
        completion: &WorkerCompletion,
    ) -> Result<bool, SchedulerError> {
        let Some(node) = self.nodes.get(&completion.node_id) else {
            return Ok(false);
        };
        let classification = classify_node_output(&completion.output);
        let (accumulated_delay, equivalent_failures, no_progress) =
            store.retry_history_totals(run_id, &completion.node_id)?;
        let retry_after_ms = retry_after_ms(&completion.output);
        match decide_retry(
            run_id,
            node,
            completion.attempt,
            classification,
            now_ms(),
            store.run_created_at_ms(run_id)?,
            accumulated_delay,
            equivalent_failures,
            no_progress,
            retry_after_ms,
        ) {
            RetryDecision::Schedule(schedule) => {
                store.schedule_retry(&schedule)?;
                Ok(true)
            }
            RetryDecision::Stop { .. } => Ok(false),
        }
    }

    fn maximum_scheduler_steps(&self) -> usize {
        self.nodes
            .values()
            .map(|node| {
                let attempts = node.retry_policy.max_attempts.max(1);
                let iterations = node
                    .loop_policy
                    .as_ref()
                    .and_then(|policy| policy.max_iterations)
                    .unwrap_or(1);
                attempts.saturating_mul(iterations) as usize
            })
            .sum::<usize>()
            .saturating_mul(4)
            .max(1)
    }

    fn advance_loop_node(
        &mut self,
        store: &mut AgentGraphStore,
        run_id: &str,
        completion: &WorkerCompletion,
    ) -> Result<(), SchedulerError> {
        let node = self
            .nodes
            .get(&completion.node_id)
            .expect("loop node exists")
            .clone();
        let mut state = store
            .loop_state(run_id, &completion.node_id)?
            .unwrap_or(LoopState::new(store.run_created_at_ms(run_id)?));
        match advance_loop(
            node.loop_policy.as_ref(),
            &mut state,
            &completion.output,
            &completion.usage,
            &self.statuses,
            now_ms(),
        )
        .map_err(|error| WorkerError::Rejected {
            node_id: completion.node_id.clone(),
            reason: error.to_string(),
        })? {
            LoopDecision::Continue => {
                store.save_loop_state(run_id, &completion.node_id, &state)?;
                store.mark_node_status(run_id, &completion.node_id, NodeStatus::Ready)?;
                self.statuses
                    .insert(completion.node_id.clone(), NodeStatus::Ready);
            }
            LoopDecision::Complete => {
                store.save_loop_state(run_id, &completion.node_id, &state)?;
            }
            LoopDecision::Partial => {
                store.save_loop_state(run_id, &completion.node_id, &state)?;
                self.partial_completion = true;
                store.mark_run_status(run_id, RunStatus::PartiallyCompleted)?;
            }
        }
        Ok(())
    }

    fn maybe_start_compensation(
        &mut self,
        store: &mut AgentGraphStore,
        run_id: &str,
    ) -> Result<(), SchedulerError> {
        let requires_compensation = self.statuses.iter().any(|(node_id, status)| {
            matches!(status, NodeStatus::Failed | NodeStatus::TimedOut)
                && self.nodes.get(node_id).is_some_and(|node| {
                    node.failure_policy == super::types::FailurePolicy::Compensate
                })
        });
        if !requires_compensation {
            return Ok(());
        }
        let plan = build_compensation_plan(&self.spec, &self.completion_order);
        if plan.steps.is_empty() {
            return Ok(());
        }
        for step in &plan.steps {
            self.statuses
                .insert(step.compensation_node_id.clone(), NodeStatus::Blocked);
            store.mark_node_status(run_id, &step.compensation_node_id, NodeStatus::Blocked)?;
        }
        store.save_compensation_plan(run_id, &plan)?;
        self.compensation_plan = Some(plan);
        self.run_status = RunStatus::Compensating;
        Ok(())
    }

    fn finish_compensation_step(
        &mut self,
        store: &mut AgentGraphStore,
        run_id: &str,
        completion: &WorkerCompletion,
    ) -> Result<(), SchedulerError> {
        let Some(plan) = self.compensation_plan.as_mut() else {
            return Ok(());
        };
        if completion.output.status == NodeStatus::Succeeded {
            plan.mark_completed();
        } else {
            plan.mark_failed();
        }
        store.save_compensation_plan(run_id, plan)?;
        self.run_status = match plan.status {
            CompensationPlanStatus::Completed => RunStatus::Compensated,
            CompensationPlanStatus::Failed => RunStatus::CompensationFailed,
            CompensationPlanStatus::ManualInterventionRequired => {
                RunStatus::ManualInterventionRequired
            }
            CompensationPlanStatus::Pending | CompensationPlanStatus::Running => {
                RunStatus::Compensating
            }
        };
        Ok(())
    }

    fn acquire_provider_admission(
        &mut self,
        backend: &SubagentWorkerBackend,
        node: &NodeSpec,
        estimate: &UsageAccounting,
    ) -> Result<bool, SchedulerError> {
        let Some(route) = backend.provider_route_for_node(node, &self.spec.spec.defaults)? else {
            return Ok(true);
        };
        if !self.provider_capacity.contains_route(&route) {
            self.provider_capacity
                .configure(route.clone(), ProviderCapacityConfig::default());
        }
        let timestamp = now_ms();
        self.provider_capacity.tick(timestamp);
        let estimated_tokens = estimate
            .input_tokens
            .saturating_add(estimate.output_tokens)
            .max(1);
        match self
            .provider_capacity
            .reserve(&route, estimated_tokens, timestamp)
        {
            ProviderAdmission::Admitted(permit) => {
                self.provider_routes.insert(node.id.clone(), route);
                self.provider_permits.insert(node.id.clone(), permit);
                Ok(true)
            }
            ProviderAdmission::WaitUntil(_) | ProviderAdmission::Saturated => Ok(false),
            ProviderAdmission::CircuitOpen => Err(SchedulerError::Admission {
                node_id: node.id.clone(),
                reason: "provider authentication circuit is open".to_string(),
            }),
        }
    }

    fn release_provider_admission(
        &mut self,
        node_id: &str,
        actual_tokens: Option<u64>,
    ) -> Option<ProviderRouteKey> {
        if let Some(permit) = self.provider_permits.remove(node_id) {
            self.provider_capacity.release(permit, actual_tokens);
        }
        self.provider_routes.remove(node_id)
    }

    fn release_all_provider_admission(&mut self) {
        let node_ids = self.provider_permits.keys().cloned().collect::<Vec<_>>();
        for node_id in node_ids {
            self.release_provider_admission(&node_id, None);
        }
    }

    fn acquire_host_admission(
        &mut self,
        run_id: &str,
        node: &NodeSpec,
        attempt: u32,
    ) -> Result<bool, SchedulerError> {
        if self.host_tickets.contains_key(&node.id) {
            return Ok(true);
        }
        let request_id = AdmissionRequestId {
            run_id: run_id.to_string(),
            node_id: node.id.clone(),
            attempt,
        };
        let mut claims = BTreeMap::new();
        if node.is_model_worker() {
            claims.insert("model-calls".to_string(), 1);
        }
        claims.insert("local-execution".to_string(), 1);
        if node.effective_capability(&self.spec.spec.defaults)
            == super::types::CapabilityMode::UnisolatedWrite
        {
            claims.insert("unisolated-writers".to_string(), 1);
        }
        for claim in &node.resource_claims {
            claims.insert(format!("resource:{}", claim.resource), claim.amount);
        }
        let class = if node.kind == NodeKind::Compensation {
            AdmissionClass::Compensation
        } else {
            match self.spec.spec.execution.orchestration_policy {
                super::types::OrchestrationMode::Standard => AdmissionClass::Graph,
                super::types::OrchestrationMode::Ultra => AdmissionClass::Ultra,
                super::types::OrchestrationMode::Swarm => AdmissionClass::Swarm,
            }
        };
        let request = AdmissionRequest {
            id: request_id.clone(),
            class,
            claims,
        };
        let mut controller =
            global_admission_controller()
                .lock()
                .map_err(|_| SchedulerError::Admission {
                    node_id: node.id.clone(),
                    reason: "host admission controller lock was poisoned".to_string(),
                })?;
        controller.ensure_capacity(
            "model-calls",
            self.spec
                .spec
                .execution
                .max_active_model_calls
                .saturating_add(1),
        );
        controller.ensure_capacity("local-execution", 33);
        for (resource_id, definition) in &self.spec.spec.resources {
            controller.ensure_capacity(format!("resource:{resource_id}"), definition.limit);
        }
        match controller.submit(request) {
            AdmissionResult::Acquired(ticket) => {
                self.host_requests.remove(&node.id);
                self.host_tickets.insert(node.id.clone(), ticket);
                Ok(true)
            }
            AdmissionResult::Queued => {
                self.host_requests.insert(node.id.clone(), request_id);
                Ok(false)
            }
            AdmissionResult::QueueFull => Err(SchedulerError::Admission {
                node_id: node.id.clone(),
                reason: "bounded host admission queue is full".to_string(),
            }),
            AdmissionResult::Impossible { resource } => Err(SchedulerError::Admission {
                node_id: node.id.clone(),
                reason: format!("resource `{resource}` request exceeds host capacity"),
            }),
        }
    }

    fn release_host_admission(&mut self, node_id: &str) {
        if let Ok(mut controller) = global_admission_controller().lock() {
            if let Some(ticket) = self.host_tickets.remove(node_id) {
                controller.release(ticket);
            }
            if let Some(request_id) = self.host_requests.remove(node_id) {
                controller.cancel(&request_id);
            }
        }
    }

    fn release_all_host_admission(&mut self) {
        let node_ids = self
            .host_tickets
            .keys()
            .chain(self.host_requests.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for node_id in node_ids {
            self.release_host_admission(&node_id);
        }
    }
}

fn run_is_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed
            | RunStatus::PartiallyCompleted
            | RunStatus::Failed
            | RunStatus::BudgetStopped
            | RunStatus::Cancelled
            | RunStatus::Compensated
            | RunStatus::CompensationFailed
            | RunStatus::ManualInterventionRequired
    )
}

fn retry_after_ms(output: &NodeOutput) -> Option<u64> {
    output
        .blockers
        .iter()
        .flat_map(|value| value.split_whitespace())
        .find_map(|part| {
            part.strip_prefix("retry-after-ms=")
                .and_then(|value| value.parse().ok())
        })
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
            | NodeKind::Loop
            | NodeKind::Compensation
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
        provider_capacity: None,
    }
}
