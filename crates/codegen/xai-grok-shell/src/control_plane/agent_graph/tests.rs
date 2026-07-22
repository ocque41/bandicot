use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tempfile::tempdir;
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend as ExistingSubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentCancelOutcome, SubagentDescribeOutcome, SubagentRequest, SubagentResult,
    SubagentSnapshot, SubagentSnapshotStatus, SubagentValidateTypeOutcome,
};
use xai_tool_runtime::ToolError;
use xai_tool_types::SubagentServiceTierPreference;

use super::*;

#[test]
fn exact_100_fake_benchmark_reaches_full_width() {
    let report = run_exact_100_fake_benchmark(100).expect("fake benchmark runs");
    assert_eq!(report.total_worker_nodes, 100);
    assert_eq!(report.configured_limit, 100);
    assert_eq!(report.peak_active_workers, 100);
    assert_eq!(report.queued_workers, 0);
    assert_eq!(report.completed_workers, 100);
    assert_eq!(report.failed_workers, 0);
    assert_eq!(report.timed_out_workers, 0);
    assert_eq!(report.cancelled_workers, 0);
    assert!(report.fake_backend);
}

#[test]
fn exact_100_fake_benchmark_queues_when_capped() {
    let report = run_exact_100_fake_benchmark(25).expect("fake benchmark runs");
    assert_eq!(report.total_worker_nodes, 100);
    assert_eq!(report.configured_limit, 25);
    assert_eq!(report.peak_active_workers, 25);
    assert_eq!(report.queued_workers, 75);
    assert_eq!(report.completed_workers, 100);
    assert_eq!(report.failed_workers, 0);
    assert_eq!(report.timed_out_workers, 0);
    assert_eq!(report.cancelled_workers, 0);
}

#[test]
fn scheduler_budget_reservations_stop_excess_ready_workers_before_dispatch() {
    let mut graph = build_exact_100_worker_graph("budget-subset");
    graph.spec.budgets.max_model_calls = Some(10);
    let mut scheduler = AgentGraphScheduler::new(
        graph,
        SchedulerConfig {
            max_active_model_calls: 100,
            plan_mode: false,
        },
    )
    .expect("scheduler");
    let mut backend = FakeWorkerBackend::complete_immediately();
    let report = scheduler
        .run_to_completion(&mut backend, "budget-subset")
        .expect("budget stop is a report, not a scheduler error");
    assert_eq!(report.run_status, RunStatus::BudgetStopped);
    assert_eq!(report.completed_nodes, 10);
    assert_eq!(report.budget.charged.model_calls, 10);
    assert!(report.budget.stopped);
}

#[test]
fn exact_100_profile_matches_worker_inventory_contract() {
    let graph = build_exact_100_worker_graph("profile");
    assert_eq!(graph.spec.nodes.len(), 100);
    assert!(graph.spec.objective.contains("repository modules"));
    assert_eq!(
        graph.spec.defaults.model_selector.as_deref(),
        Some("worker-light")
    );
    assert!(graph.spec.model_selectors.contains("worker-light"));
    assert!(graph.spec.model_selectors.contains("luna"));
    assert!(
        graph
            .spec
            .templates
            .contains("overhead-role:root-coordinator")
    );
    assert!(
        graph
            .spec
            .templates
            .contains("overhead-role:result-verifier")
    );
    assert_eq!(
        graph.spec.defaults.service_tier,
        ServiceTierPreference::Standard
    );
    for node in &graph.spec.nodes {
        assert_eq!(node.service_tier, Some(ServiceTierPreference::Standard));
        assert_eq!(node.max_input_tokens, Some(20_000));
        assert_eq!(node.max_output_tokens, Some(5_000));
        assert_eq!(node.max_tool_calls, Some(20));
        assert_eq!(node.timeout_seconds, Some(600));
    }
}

#[test]
fn exact_100_validation_rejects_99_and_chained_graphs() {
    let mut ninety_nine = build_exact_100_worker_graph("ninety-nine");
    ninety_nine.spec.nodes.pop();
    assert!(matches!(
        validate_exact_100_worker_graph(&ninety_nine),
        Err(BenchmarkError::Validation(errors)) if errors.iter().any(|error| matches!(error, ValidationError::ExactReadyMismatch { required: 100, actual: 99 }))
    ));

    let mut chained = build_exact_100_worker_graph("chained");
    chained.spec.edges.push(EdgeSpec {
        id: Some("chain-000-001".to_string()),
        from: "worker-000".to_string(),
        to: "worker-001".to_string(),
        kind: EdgeKind::Control,
        bindings: Vec::new(),
        join: None,
        condition: None,
    });
    assert!(matches!(
        validate_exact_100_worker_graph(&chained),
        Err(BenchmarkError::Validation(errors)) if errors.iter().any(|error| matches!(error, ValidationError::ExactReadyMismatch { required: 100, actual: 99 }))
    ));
}

#[test]
fn resource_manager_blocks_write_conflicts_and_plan_mode_writes() {
    let mut graph = build_exact_100_worker_graph("resources");
    graph.spec.nodes.truncate(2);
    graph.spec.execution.max_total_nodes = 2;
    graph.spec.nodes[0].id = "writer-a".to_string();
    graph.spec.nodes[0].write_set = vec!["src/**".to_string()];
    graph.spec.nodes[0].capability_mode = Some(CapabilityMode::WorktreeWrite);
    graph.spec.nodes[1].id = "writer-b".to_string();
    graph.spec.nodes[1].write_set = vec!["src/lib.rs".to_string()];
    graph.spec.nodes[1].capability_mode = Some(CapabilityMode::WorktreeWrite);

    let mut manager = ResourceManager::from_graph(&graph);
    let ticket = manager
        .try_acquire(&graph.spec.nodes[0], &graph.spec.defaults, false)
        .expect("first writer acquires");
    assert!(matches!(
        manager.try_acquire(&graph.spec.nodes[1], &graph.spec.defaults, false),
        Err(ResourceError::WriteSetConflict { .. })
    ));
    manager.release(ticket);
    assert!(
        manager
            .try_acquire(&graph.spec.nodes[1], &graph.spec.defaults, false)
            .is_ok()
    );

    let mut manager = ResourceManager::from_graph(&graph);
    assert!(matches!(
        manager.try_acquire(&graph.spec.nodes[0], &graph.spec.defaults, true),
        Err(ResourceError::PlanModeWriter { .. })
    ));
}

#[test]
fn scheduler_queues_resource_contention_instead_of_failing() {
    let mut graph = build_exact_100_worker_graph("scheduler-contention");
    graph.spec.nodes.truncate(2);
    graph.spec.execution.max_total_nodes = 2;
    graph.spec.execution.max_active_model_calls = 2;
    graph.spec.nodes[0].id = "writer-a".to_string();
    graph.spec.nodes[0].write_set = vec!["src/**".to_string()];
    graph.spec.nodes[0].capability_mode = Some(CapabilityMode::WorktreeWrite);
    graph.spec.nodes[1].id = "writer-b".to_string();
    graph.spec.nodes[1].write_set = vec!["src/lib.rs".to_string()];
    graph.spec.nodes[1].capability_mode = Some(CapabilityMode::WorktreeWrite);

    let mut scheduler = AgentGraphScheduler::new(
        graph,
        SchedulerConfig {
            max_active_model_calls: 2,
            plan_mode: false,
        },
    )
    .expect("valid graph");
    let mut backend = FakeWorkerBackend::complete_immediately();
    let report = scheduler
        .run_to_completion(&mut backend, "resource-contention")
        .expect("contention queues");
    assert_eq!(report.run_status, RunStatus::Completed);
    assert_eq!(report.completed_nodes, 2);
    assert_eq!(report.failed_nodes, 0);
    assert_eq!(report.queued_workers, 1);
}

#[test]
fn worker_isolation_rejects_nested_tools() {
    let mut graph = build_exact_100_worker_graph("nested-tools");
    graph.spec.nodes.truncate(1);
    graph.spec.execution.max_total_nodes = 1;
    graph.spec.nodes[0].tool_allowlist = vec!["task".to_string()];
    assert!(matches!(
        validate_worker_isolation(&graph),
        Err(ResourceError::NestedOrchestrationTool { .. })
    ));
}

#[test]
fn store_reopens_replays_and_rejects_late_stale_attempt_output() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("agentgraph.db");
    let mut graph = build_exact_100_worker_graph("store");
    graph.spec.nodes.truncate(1);
    graph.spec.execution.max_total_nodes = 1;
    graph.spec.execution.max_active_model_calls = 1;

    let run_id = {
        let mut store = AgentGraphStore::open(&db_path).expect("store opens");
        let run_id = store
            .create_run(&graph, Some("session-1"), dir.path())
            .expect("run created");
        assert_eq!(
            store.run_status(&run_id).expect("status"),
            RunStatus::AwaitingApproval
        );
        store
            .mark_run_status(&run_id, RunStatus::Running)
            .expect("run marked running");
        let lease = store
            .lease_node(&run_id, "worker-000", "owner-a", 1)
            .expect("lease query")
            .expect("lease granted");
        assert_eq!(lease.attempt, 1);
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(store.expire_leases(store::now_ms()).expect("expired"), 1);
        let stale_output = node_output(&run_id, "worker-000", 1, NodeStatus::Succeeded);
        assert!(
            store
                .accept_output(&run_id, "worker-000", 1, &stale_output)
                .expect("stale output checked"),
            "same attempt can still be accepted before a replacement lease"
        );
        run_id
    };

    let store = AgentGraphStore::open(&db_path).expect("store reopens");
    let snapshot = store.replay_run(&run_id).expect("snapshot");
    assert_eq!(snapshot.node_states.len(), 1);
    assert!(snapshot.events.len() >= 4);
}

#[test]
fn store_rejects_output_from_superseded_attempt() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("agentgraph.db");
    let mut graph = build_exact_100_worker_graph("store-stale");
    graph.spec.nodes.truncate(1);
    graph.spec.execution.max_total_nodes = 1;
    graph.spec.execution.max_active_model_calls = 1;

    let mut store = AgentGraphStore::open(&db_path).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let first = store
        .lease_node(&run_id, "worker-000", "owner-a", 1)
        .expect("lease query")
        .expect("first lease");
    std::thread::sleep(Duration::from_millis(2));
    store.expire_leases(store::now_ms()).expect("expired");
    let second = store
        .lease_node(&run_id, "worker-000", "owner-b", 1000)
        .expect("lease query")
        .expect("second lease");
    assert_eq!(second.attempt, first.attempt + 1);
    let stale_output = node_output(&run_id, "worker-000", first.attempt, NodeStatus::Succeeded);
    assert!(
        !store
            .accept_output(&run_id, "worker-000", first.attempt, &stale_output)
            .expect("stale rejected")
    );
}

#[test]
fn verification_rejects_unsafe_output_paths_and_out_of_scope_changes() {
    let output = NodeOutput {
        files_changed: vec!["README.md".to_string(), "../outside".to_string()],
        confidence: 1.2,
        ..node_output("run-1", "node-1", 1, NodeStatus::Running)
    };
    let report = verify_node_output(&output, Path::new("."), &["src/**".to_string()]);
    assert!(!report.is_valid());
    assert!(
        report
            .errors
            .iter()
            .any(|error| matches!(error, VerificationError::NonTerminalStatus { .. }))
    );
    assert!(
        report
            .errors
            .iter()
            .any(|error| matches!(error, VerificationError::ChangedPathOutsideWriteSet { .. }))
    );
}

#[test]
fn graph_command_validates_spec_file() {
    let dir = tempdir().expect("tempdir");
    let graph = build_exact_100_worker_graph("command-validate");
    let path = dir.path().join("graph.json");
    std::fs::write(&path, serde_json::to_string_pretty(&graph).unwrap()).unwrap();
    let output = graph_command_output(&format!("preview {}", path.display()), dir.path(), None);
    assert!(output.contains("GraphSpec is valid"), "{output}");
    assert!(output.contains("Initial ready agents: 100"), "{output}");
    assert!(output.contains("Budget:"), "{output}");
    assert!(output.contains("Models:"), "{output}");
    assert!(output.contains("Permissions:"), "{output}");
    assert!(output.contains("Effects:"), "{output}");
}

#[test]
fn graph_command_plan_run_and_pause_use_session_scoped_store() {
    let dir = tempdir().expect("tempdir");
    let mut graph = build_exact_100_worker_graph("command-run");
    graph.spec.nodes.truncate(2);
    graph.spec.execution.max_total_nodes = 2;
    graph.spec.execution.max_active_model_calls = 2;
    let path = dir.path().join("graph.json");
    std::fs::write(&path, serde_json::to_string_pretty(&graph).unwrap()).unwrap();

    let output = graph_command_output(
        &format!("plan {}", path.display()),
        dir.path(),
        Some("session-a"),
    );
    assert!(output.contains("awaiting approval"), "{output}");
    assert!(output.contains("Session: session-a"), "{output}");

    let output = graph_command_output("run", dir.path(), Some("session-a"));
    assert!(output.contains("was not executed"), "{output}");
    assert!(
        output.contains("No fake worker results were recorded"),
        "{output}"
    );

    let output = graph_command_output("pause", dir.path(), Some("session-a"));
    assert!(output.contains("Paused"), "{output}");
    let status = graph_command_output("status", dir.path(), Some("session-a"));
    assert!(status.contains("Paused"), "{status}");

    let other_status = graph_command_output("status", dir.path(), Some("session-b"));
    assert!(other_status.contains("no active run"), "{other_status}");
}

#[test]
fn swarm_plan_and_run_use_control_plane_store() {
    let dir = tempdir().expect("tempdir");
    let plan = swarm_command_output("plan --fake", dir.path(), Some("session-a"));
    assert!(plan.contains("awaiting approval"), "{plan}");
    let status = swarm_command_output("status", dir.path(), Some("session-a"));
    assert!(status.contains("AwaitingApproval"), "{status}");

    let live_run = swarm_command_output("run", dir.path(), Some("session-a"));
    assert!(live_run.contains("not executed"), "{live_run}");
    assert!(live_run.contains("unavailable"), "{live_run}");

    let run = swarm_command_output("run --fake", dir.path(), Some("session-a"));
    assert!(run.contains("Fake graph run"), "{run}");
    assert!(run.contains("Backend: fake"), "{run}");
    assert!(run.contains("Completed: 100"), "{run}");
}

#[tokio::test]
async fn real_subagent_scheduler_runs_bounded_concurrently() {
    let dir = tempdir().expect("tempdir");
    let graph = runnable_real_backend_graph("bounded-concurrency", 3, 2);
    let mut store = AgentGraphStore::open(dir.path().join("agentgraph.db")).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::StructuredSuccess,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend.clone(), "session-1", dir.path());
    let mut scheduler = AgentGraphScheduler::new(
        graph.clone(),
        SchedulerConfig {
            max_active_model_calls: 2,
            plan_mode: false,
        },
    )
    .expect("scheduler");

    let report = scheduler
        .run_to_completion_with_subagents(&worker_backend, &mut store, &run_id)
        .await
        .expect("run completes");

    assert_eq!(report.run_status, RunStatus::Completed);
    assert_eq!(report.completed_nodes, 3);
    assert_eq!(report.failed_nodes, 0);
    assert_eq!(report.peak_active_workers, 2);
    assert_eq!(report.queued_workers, 1);
    assert_eq!(backend.peak_active.load(Ordering::SeqCst), 2);
    assert_eq!(
        backend.max_seen_capability_mode(),
        Some("read-only".to_string())
    );
    assert_eq!(
        backend.max_seen_service_tier(),
        Some(SubagentServiceTierPreference::Standard)
    );
    assert_eq!(backend.max_seen_hosted_multi_agent(), Some(false));
}

#[tokio::test]
async fn real_subagent_success_plain_text_is_failed_not_synthetic_success() {
    let dir = tempdir().expect("tempdir");
    let graph = runnable_real_backend_graph("plain-text-rejected", 1, 1);
    let mut store = AgentGraphStore::open(dir.path().join("agentgraph.db")).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::PlainTextSuccess,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend, "session-1", dir.path());
    let config = SchedulerConfig::from_graph(&graph);
    let mut scheduler = AgentGraphScheduler::new(graph, config).expect("scheduler");

    let report = scheduler
        .run_to_completion_with_subagents(&worker_backend, &mut store, &run_id)
        .await
        .expect("run completes with failed node");

    assert_eq!(report.run_status, RunStatus::Failed);
    let snapshot = store.replay_run(&run_id).expect("snapshot");
    let output = snapshot.node_states[0]
        .output
        .as_ref()
        .expect("node output");
    assert_eq!(output.status, NodeStatus::Failed);
    assert!(
        output
            .blockers
            .iter()
            .any(|blocker| blocker.contains("not valid NodeOutput JSON")),
        "{output:?}"
    );
}

#[tokio::test]
async fn backgrounded_subagent_is_queried_to_terminal_output() {
    let dir = tempdir().expect("tempdir");
    let graph = runnable_real_backend_graph("background-query", 1, 1);
    let mut store = AgentGraphStore::open(dir.path().join("agentgraph.db")).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::BackgroundThenQueryComplete,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend.clone(), "session-1", dir.path());
    let config = SchedulerConfig::from_graph(&graph);
    let mut scheduler = AgentGraphScheduler::new(graph, config).expect("scheduler");

    let report = scheduler
        .run_to_completion_with_subagents(&worker_backend, &mut store, &run_id)
        .await
        .expect("run completes");

    assert_eq!(report.run_status, RunStatus::Completed);
    assert_eq!(backend.query_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_pause_and_resume_are_observed_between_batches() {
    let dir = tempdir().expect("tempdir");
    let graph = runnable_real_backend_graph("pause-resume", 2, 1);
    let db_path = dir.path().join("agentgraph.db");
    let mut store = AgentGraphStore::open(&db_path).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::StructuredSuccess,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend.clone(), "session-1", dir.path());
    let config = SchedulerConfig::from_graph(&graph);
    let mut scheduler = AgentGraphScheduler::new(graph, config).expect("scheduler");

    let control_run_id = run_id.clone();
    let control_db_path = db_path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        {
            let mut control_store = AgentGraphStore::open(&control_db_path).expect("control store");
            control_store
                .mark_run_status(&control_run_id, RunStatus::Paused)
                .expect("pause");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
        {
            let mut control_store = AgentGraphStore::open(&control_db_path).expect("control store");
            control_store
                .mark_run_status(&control_run_id, RunStatus::Running)
                .expect("resume");
        }
    });

    let report = scheduler
        .run_to_completion_with_subagents(&worker_backend, &mut store, &run_id)
        .await
        .expect("run completes");

    assert_eq!(report.run_status, RunStatus::Completed);
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn durable_cancel_stops_before_next_batch() {
    let dir = tempdir().expect("tempdir");
    let graph = runnable_real_backend_graph("cancel", 2, 1);
    let db_path = dir.path().join("agentgraph.db");
    let mut store = AgentGraphStore::open(&db_path).expect("store opens");
    let run_id = store
        .create_run(&graph, Some("session-1"), dir.path())
        .expect("run created");
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::StructuredSuccess,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend.clone(), "session-1", dir.path());
    let config = SchedulerConfig::from_graph(&graph);
    let mut scheduler = AgentGraphScheduler::new(graph, config).expect("scheduler");

    let control_run_id = run_id.clone();
    let control_db_path = db_path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut control_store = AgentGraphStore::open(&control_db_path).expect("control store");
        control_store
            .mark_run_status(&control_run_id, RunStatus::Cancelled)
            .expect("cancel");
    });

    let report = scheduler
        .run_to_completion_with_subagents(&worker_backend, &mut store, &run_id)
        .await
        .expect("run cancels");

    assert_eq!(report.run_status, RunStatus::Cancelled);
    assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn real_backend_resolves_builtin_model_selector() {
    let dir = tempdir().expect("tempdir");
    let graph = build_exact_100_worker_graph("selector-rejected");
    let node = graph.spec.nodes[0].clone();
    let backend = Arc::new(ScriptedSubagentBackend::new(
        ScriptedBackendMode::StructuredSuccess,
    ));
    let worker_backend = SubagentWorkerBackend::new(backend, "session-1", dir.path());
    let result = worker_backend
        .run_node(
            "run-1",
            &node,
            &graph.spec.defaults,
            &graph.spec.schemas,
            1,
            graph_worker_id("run-1", &node.id, 1),
        )
        .await;
    assert!(
        result.is_ok(),
        "builtin selector should resolve: {result:?}"
    );
}

fn node_output(run_id: &str, node_id: &str, attempt: u32, status: NodeStatus) -> NodeOutput {
    NodeOutput {
        schema_version: AGENTGRAPH_SCHEMA_VERSION,
        graph_run_id: run_id.to_string(),
        node_instance_id: node_id.to_string(),
        attempt_id: format!("attempt-{attempt}"),
        status,
        summary: "done".to_string(),
        findings: Vec::new(),
        files_read: Vec::new(),
        files_changed: Vec::new(),
        commands_run: Vec::new(),
        tests_run: Vec::new(),
        artifacts: Vec::new(),
        assumptions: Vec::new(),
        blockers: Vec::new(),
        confidence: 1.0,
    }
}

fn runnable_real_backend_graph(name: &str, workers: usize, max_active: u32) -> GraphSpec {
    let mut graph = build_exact_100_worker_graph(name);
    graph.spec.nodes.truncate(workers);
    graph.spec.execution.max_total_nodes = workers as u32;
    graph.spec.execution.max_active_model_calls = max_active;
    graph.spec.defaults.model_selector = None;
    graph.spec.model_selectors.clear();
    for node in &mut graph.spec.nodes {
        node.tags.clear();
        node.model_selector = None;
        node.timeout_seconds = Some(2);
    }
    graph
}

#[derive(Debug, Clone, Copy)]
enum ScriptedBackendMode {
    StructuredSuccess,
    PlainTextSuccess,
    BackgroundThenQueryComplete,
}

#[derive(Debug)]
struct ScriptedSubagentBackend {
    mode: ScriptedBackendMode,
    active: AtomicUsize,
    peak_active: AtomicUsize,
    spawn_count: AtomicUsize,
    query_count: AtomicUsize,
    max_seen_capability_mode: std::sync::Mutex<Option<String>>,
    max_seen_service_tier: std::sync::Mutex<Option<SubagentServiceTierPreference>>,
    max_seen_hosted_multi_agent: std::sync::Mutex<Option<bool>>,
    background_outputs: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl ScriptedSubagentBackend {
    fn new(mode: ScriptedBackendMode) -> Self {
        Self {
            mode,
            active: AtomicUsize::new(0),
            peak_active: AtomicUsize::new(0),
            spawn_count: AtomicUsize::new(0),
            query_count: AtomicUsize::new(0),
            max_seen_capability_mode: std::sync::Mutex::new(None),
            max_seen_service_tier: std::sync::Mutex::new(None),
            max_seen_hosted_multi_agent: std::sync::Mutex::new(None),
            background_outputs: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    fn max_seen_capability_mode(&self) -> Option<String> {
        self.max_seen_capability_mode.lock().expect("lock").clone()
    }

    fn max_seen_service_tier(&self) -> Option<SubagentServiceTierPreference> {
        *self.max_seen_service_tier.lock().expect("lock")
    }

    fn max_seen_hosted_multi_agent(&self) -> Option<bool> {
        *self.max_seen_hosted_multi_agent.lock().expect("lock")
    }
}

#[async_trait::async_trait]
impl ExistingSubagentBackend for ScriptedSubagentBackend {
    async fn spawn(&self, request: SubagentRequest) -> Result<SubagentResult, ToolError> {
        self.spawn_count.fetch_add(1, Ordering::SeqCst);
        if let Some(mode) = request.runtime_overrides.capability_mode {
            *self.max_seen_capability_mode.lock().expect("lock") = Some(mode.as_str().to_string());
        }
        *self.max_seen_service_tier.lock().expect("lock") = request.runtime_overrides.service_tier;
        *self.max_seen_hosted_multi_agent.lock().expect("lock") =
            request.runtime_overrides.hosted_multi_agent;
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_active.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);

        match self.mode {
            ScriptedBackendMode::StructuredSuccess => Ok(SubagentResult {
                success: true,
                output: Arc::from(node_output_json_from_prompt(&request.prompt)),
                error: None,
                cancelled: false,
                subagent_id: request.id.clone(),
                child_session_id: request.id,
                tool_calls: 1,
                turns: 1,
                duration_ms: 20,
                tokens_used: 10,
                worktree_path: None,
                backgrounded: false,
            }),
            ScriptedBackendMode::PlainTextSuccess => Ok(SubagentResult {
                success: true,
                output: Arc::from("plain text is not a NodeOutput"),
                error: None,
                cancelled: false,
                subagent_id: request.id.clone(),
                child_session_id: request.id,
                tool_calls: 1,
                turns: 1,
                duration_ms: 20,
                tokens_used: 10,
                worktree_path: None,
                backgrounded: false,
            }),
            ScriptedBackendMode::BackgroundThenQueryComplete => Ok(SubagentResult {
                success: true,
                output: Arc::from(""),
                error: None,
                cancelled: false,
                subagent_id: request.id.clone(),
                child_session_id: {
                    self.background_outputs.lock().expect("lock").insert(
                        request.id.clone(),
                        node_output_json_from_prompt(&request.prompt),
                    );
                    request.id
                },
                tool_calls: 0,
                turns: 0,
                duration_ms: 1,
                tokens_used: 0,
                worktree_path: None,
                backgrounded: true,
            }),
        }
    }

    async fn query(
        &self,
        id: &str,
        _block: bool,
        _timeout_ms: Option<u64>,
    ) -> Option<SubagentSnapshot> {
        self.query_count.fetch_add(1, Ordering::SeqCst);
        let output = self
            .background_outputs
            .lock()
            .expect("lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| node_output_json("unknown-run", "unknown-node", 1));
        Some(SubagentSnapshot {
            subagent_id: id.to_string(),
            description: "backgrounded graph worker".to_string(),
            subagent_type: "general-purpose".to_string(),
            status: SubagentSnapshotStatus::Completed {
                output,
                tool_calls: 1,
                turns: 1,
                worktree_path: None,
            },
            started_at_epoch_ms: 0,
            duration_ms: 20,
            persona: None,
        })
    }

    async fn cancel(&self, _id: &str) -> SubagentCancelOutcome {
        SubagentCancelOutcome::Cancelled
    }

    async fn validate_type(
        &self,
        _subagent_type: &str,
        _parent_session_id: &str,
    ) -> SubagentValidateTypeOutcome {
        SubagentValidateTypeOutcome::Ok
    }

    async fn describe_subagent_type(
        &self,
        _subagent_type: &str,
        _harness_agent_type: Option<&str>,
        _parent_session_id: &str,
    ) -> SubagentDescribeOutcome {
        SubagentDescribeOutcome::Unavailable
    }
}

fn node_output_json_from_prompt(prompt: &str) -> String {
    let run_id = prompt_line_value(prompt, "Graph run: ").unwrap_or("unknown-run");
    let node_id = prompt_line_value(prompt, "Node id: ").unwrap_or("unknown-node");
    let attempt = prompt_line_value(prompt, "Attempt: ")
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(1);
    node_output_json(run_id, node_id, attempt)
}

fn node_output_json(run_id: &str, node_id: &str, attempt: u32) -> String {
    serde_json::to_string(&node_output(
        run_id,
        node_id,
        attempt,
        NodeStatus::Succeeded,
    ))
    .expect("serialize NodeOutput")
}

fn prompt_line_value<'a>(prompt: &'a str, prefix: &str) -> Option<&'a str> {
    prompt
        .lines()
        .find_map(|line| line.strip_prefix(prefix).map(str::trim))
}
