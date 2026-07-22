use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use xai_acp_lib::AcpAgentGatewaySender as GatewaySender;
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend;

use crate::control_plane::agent_graph::{
    AgentGraphControlPlane, AgentGraphService, AgentGraphStore, ApprovalBinding, ExecutionApproval,
    GraphSpec, RunStatus, graph_requires_execution_approval, resolve_repository_commit,
};

use super::ExtResult;

pub const PREFIX: &str = "x.ai/graph/";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRequest {
    repo_root: PathBuf,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    spec: Option<GraphSpec>,
    #[serde(default)]
    approval: Option<ExecutionApproval>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunResponse {
    accepted: bool,
    status: crate::control_plane::agent_graph::service::GraphStatusDto,
}

pub async fn handle(
    args: &acp::ExtRequest,
    backend: Arc<dyn SubagentBackend>,
    models_manager: crate::agent::models::ModelsManager,
    gateway: GatewaySender,
) -> ExtResult {
    if !args.method.as_ref().starts_with(PREFIX) {
        return Err(acp::Error::method_not_found());
    }
    let request: GraphRequest = super::parse_params(args)?;
    let service = AgentGraphService::new(&request.repo_root);
    match args.method.as_ref() {
        "x.ai/graph/draft" => {
            let spec = require_spec(request.spec)?;
            notified(
                service.draft(&spec, request.session_id.as_deref()),
                &gateway,
                &["x.ai/graph/updated"],
            )
        }
        "x.ai/graph/validate" => {
            let spec = require_spec(request.spec)?;
            raw(Ok(service.validate(&spec)))
        }
        "x.ai/graph/preview" => {
            let spec = require_spec(request.spec)?;
            raw(service.preview(&spec))
        }
        "x.ai/graph/status" | "x.ai/graph/artifact" => notified(
            service
                .status(require_run_id(&request)?)
                .map_err(|error| error.to_string()),
            &gateway,
            &[
                "x.ai/graph/nodeUpdated",
                "x.ai/graph/budgetUpdated",
                "x.ai/graph/rateLimitUpdated",
            ],
        ),
        "x.ai/graph/explain" => {
            let run_id = require_run_id(&request)?;
            let store = AgentGraphStore::open(request.repo_root.join(".agent/agentgraph.db"))
                .map_err(internal)?;
            let spec = store.graph_spec_for_run(run_id).map_err(internal)?;
            let preview = service.preview(&spec).map_err(invalid)?;
            let status = service.status(run_id).map_err(internal)?;
            raw(Ok(
                serde_json::json!({ "preview": preview, "status": status }),
            ))
        }
        "x.ai/graph/pause" => transition(&service, &request, RunStatus::Paused, &gateway),
        "x.ai/graph/drain" => transition(&service, &request, RunStatus::Draining, &gateway),
        "x.ai/graph/resume" => transition(&service, &request, RunStatus::Running, &gateway),
        "x.ai/graph/cancel" => transition(&service, &request, RunStatus::Cancelled, &gateway),
        "x.ai/graph/retry" => notified(
            service
                .retry_failed(require_run_id(&request)?)
                .map_err(|error| error.to_string()),
            &gateway,
            &["x.ai/graph/updated", "x.ai/graph/nodeUpdated"],
        ),
        "x.ai/graph/export" => raw(service
            .export(require_run_id(&request)?)
            .map_err(|error| error.to_string())),
        "x.ai/graph/cleanup" => {
            service
                .cleanup(require_run_id(&request)?)
                .map_err(internal)?;
            raw(Ok(serde_json::json!({ "cleaned": true })))
        }
        "x.ai/graph/run" => run(request, service, backend, models_manager, gateway).await,
        _ => Err(acp::Error::method_not_found()),
    }
}

async fn run(
    request: GraphRequest,
    service: AgentGraphService,
    backend: Arc<dyn SubagentBackend>,
    models_manager: crate::agent::models::ModelsManager,
    gateway: GatewaySender,
) -> ExtResult {
    let run_id = if let Some(spec) = request.spec.as_ref() {
        service
            .draft(spec, request.session_id.as_deref())
            .map_err(invalid)?
            .run_id
    } else {
        require_run_id(&request)?.to_string()
    };
    let store_path = request.repo_root.join(".agent/agentgraph.db");
    let mut store = AgentGraphStore::open(&store_path).map_err(internal)?;
    let spec = store.graph_spec_for_run(&run_id).map_err(internal)?;
    let commit = resolve_repository_commit(&request.repo_root).map_err(internal)?;
    if graph_requires_execution_approval(&spec) {
        let Some(approval) = request.approval.as_ref() else {
            let required = ApprovalBinding::for_spec(
                &spec,
                &commit,
                crate::control_plane::agent_graph::store::now_ms() + 15 * 60 * 1_000,
            )
            .map_err(internal)?;
            notify(
                &gateway,
                "x.ai/graph/approvalRequired",
                &serde_json::json!({
                    "schemaVersion": crate::control_plane::agent_graph::service::GRAPH_ACP_SCHEMA_VERSION,
                    "runId": run_id,
                    "requiredApproval": required,
                }),
            );
            return Err(acp::Error::invalid_params().data(
                serde_json::json!({
                    "code": "approval_required",
                    "runId": run_id,
                    "requiredApproval": required,
                })
                .to_string(),
            ));
        };
        approval
            .binding
            .verify(
                &spec,
                &commit,
                crate::control_plane::agent_graph::store::now_ms(),
            )
            .map_err(invalid)?;
        store
            .save_execution_approval(&run_id, approval)
            .map_err(internal)?;
    }
    let session_id = request
        .session_id
        .as_deref()
        .ok_or_else(|| acp::Error::invalid_params().data("sessionId is required to run"))?;
    store
        .attach_active_run(session_id, &request.repo_root, &run_id)
        .map_err(internal)?;
    let control = AgentGraphControlPlane::new(&request.repo_root, Some(session_id))
        .with_models_manager(models_manager);
    let outcome = control.run_active_with_backend(backend);
    if outcome.contains("was not started") || outcome.contains("failed") {
        return Err(acp::Error::internal_error().data(outcome));
    }
    notified(
        service
            .status(&run_id)
            .map(|status| RunResponse {
                accepted: true,
                status,
            })
            .map_err(|error| error.to_string()),
        &gateway,
        &[
            "x.ai/graph/updated",
            "x.ai/graph/nodeUpdated",
            "x.ai/graph/budgetUpdated",
            "x.ai/graph/rateLimitUpdated",
        ],
    )
}

fn transition(
    service: &AgentGraphService,
    request: &GraphRequest,
    status: RunStatus,
    gateway: &GatewaySender,
) -> ExtResult {
    notified(
        service
            .transition(require_run_id(request)?, status)
            .map_err(|error| error.to_string()),
        gateway,
        &["x.ai/graph/updated", "x.ai/graph/nodeUpdated"],
    )
}

fn require_spec(spec: Option<GraphSpec>) -> Result<GraphSpec, acp::Error> {
    spec.ok_or_else(|| acp::Error::invalid_params().data("spec is required"))
}

fn require_run_id(request: &GraphRequest) -> Result<&str, acp::Error> {
    request
        .run_id
        .as_deref()
        .ok_or_else(|| acp::Error::invalid_params().data("runId is required"))
}

fn raw<T: Serialize>(result: Result<T, String>) -> ExtResult {
    result
        .map_err(invalid)
        .and_then(|value| super::to_raw_response(&value))
}

fn notified<T: Serialize>(
    result: Result<T, String>,
    gateway: &GatewaySender,
    methods: &[&str],
) -> ExtResult {
    let value = result.map_err(invalid)?;
    for method in methods {
        notify(gateway, method, &value);
    }
    super::to_raw_response(&value)
}

fn notify<T: Serialize>(gateway: &GatewaySender, method: &str, value: &T) {
    if let Ok(raw) = serde_json::value::to_raw_value(value) {
        gateway.forward_fire_and_forget(acp::ExtNotification::new(method, raw.into()));
    }
}

fn invalid(error: impl ToString) -> acp::Error {
    acp::Error::invalid_params().data(error.to_string())
}

fn internal(error: impl ToString) -> acp::Error {
    acp::Error::internal_error().data(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::agent_graph::build_exact_100_worker_graph;

    #[test]
    fn graph_request_round_trips_structured_spec() {
        let value = serde_json::json!({
            "repoRoot": "/tmp/repo",
            "sessionId": "s1",
            "spec": build_exact_100_worker_graph("round-trip"),
        });
        let request: GraphRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.session_id.as_deref(), Some("s1"));
        assert_eq!(request.spec.unwrap().metadata.name, "round-trip");
    }

    #[test]
    fn method_names_are_versioned_under_one_prefix() {
        for method in [
            "draft", "validate", "preview", "run", "status", "explain", "pause", "drain", "resume",
            "cancel", "retry", "artifact", "export", "cleanup",
        ] {
            assert!(format!("{PREFIX}{method}").starts_with("x.ai/graph/"));
        }
    }
}
