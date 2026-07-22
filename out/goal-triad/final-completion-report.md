# Bandicot Control-Plane Final Completion Report

Date: 2026-07-22

Status: locally complete and testable. Live authenticated provider and Codex account checks remain external-only.

## Architecture

Fast is a requested service-tier preference resolved against the current provider/model and serialized only when supported. Ultra is a root-session orchestration policy with a depth-one, six-child ceiling. AgentGraph is the shared durable control plane: one canonical service sits above SQLite state, atomic budgets and leases, fair admission, typed retries, loop state, compensation, provider capacity, model resolution, worker isolation, approvals, and structured outputs. Slash commands, ACP methods, and headless callers use this service. Swarm is an AgentGraph execution profile with a conservative canary ramp and hard opt-in gates.

## Closed gaps

- Added typed settings and complete modal persistence/action/rollback plumbing.
- Connected live Swarm to the existing session subagent backend and prevented fake fallback.
- Added durable atomic budget reservation/reconciliation and conservative missing-usage charging.
- Added startup ownership, recovery, lease expiry, wall deadlines, and retry activation.
- Added persisted typed retry schedules with deterministic jitter and Retry-After.
- Added bounded loop state and deterministic expansion identifiers.
- Added reverse-order persisted compensation.
- Added fair multi-resource admission, head-of-line bypass, and interactive reserve.
- Added adaptive provider request/token/project-token control and child-session metadata transfer.
- Added live model-catalog selector resolution and dispatch-time re-resolution.
- Added all required structured ACP methods and five lifecycle notifications.
- Added immutable headless approval bindings.
- Completed local Codex app-server adapter behavior and fake lifecycle coverage; live authenticated smoke is external-only.
- Enforced hosted multi-agent and nested orchestration isolation for workers.
- Added prompt-cache capability gating and separate cached/cache-write accounting.
- Replaced preview/status placeholders with canonical calculated output.
- Updated the control-plane guide, user guide, README, and changelog.

## Result

Gap matrix: PASS 20, FAIL 0, EXTERNAL-ONLY 1.

The local mock live-Swarm path completed 100 of 100 workers. The exact-100 fake benchmark reached peak concurrency 100. All focused compile and test suites passed. Strict clippy still reports unrelated pre-existing lints, recorded in the verification log.

## Exact files changed in this completion pass

```text
CHANGELOG.md
README.md
crates/codegen/xai-grok-pager/docs/user-guide/04-slash-commands.md
crates/codegen/xai-grok-pager/docs/user-guide/05-configuration.md
crates/codegen/xai-grok-pager/docs/user-guide/14-headless-mode.md
crates/codegen/xai-grok-pager/docs/user-guide/22-permissions-and-safety.md
crates/codegen/xai-grok-pager/src/app/actions.rs
crates/codegen/xai-grok-pager/src/app/dispatch/dashboard.rs
crates/codegen/xai-grok-pager/src/app/dispatch/prompt.rs
crates/codegen/xai-grok-pager/src/app/dispatch/router.rs
crates/codegen/xai-grok-pager/src/app/dispatch/settings/setters.rs
crates/codegen/xai-grok-pager/src/app/dispatch/settings/ui.rs
crates/codegen/xai-grok-pager/src/app/dispatch/tests/settings.rs
crates/codegen/xai-grok-pager/src/app/effects/helpers.rs
crates/codegen/xai-grok-pager/src/settings/defs.rs
crates/codegen/xai-grok-pager/src/settings/registry.rs
crates/codegen/xai-grok-pager/src/views/settings_modal/state.rs
crates/codegen/xai-grok-pager/src/views/settings_modal/tests.rs
crates/codegen/xai-grok-sampler/src/client.rs
crates/codegen/xai-grok-sampler/src/stream/chat_completions.rs
crates/codegen/xai-grok-sampling-types/src/error.rs
crates/codegen/xai-grok-sampling-types/src/lib.rs
crates/codegen/xai-grok-sampling-types/src/service_tier.rs
crates/codegen/xai-grok-shell/src/agent/config.rs
crates/codegen/xai-grok-shell/src/agent/models.rs
crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/admission.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/budget.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/codex_app_server.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/commands.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/compensation.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/loop_controller.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/model_selector.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/mod.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/rate_limit.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/retry.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/runtime.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/scheduler.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/service.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/store.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/tests.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/types.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/validation.rs
crates/codegen/xai-grok-shell/src/control_plane/agent_graph/worker.rs
crates/codegen/xai-grok-shell/src/extensions/agent_graph.rs
crates/codegen/xai-grok-shell/src/extensions/mod.rs
crates/codegen/xai-grok-shell/src/session/acp_session.rs
crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs
crates/codegen/xai-grok-shell/src/session/acp_session_impl/session_setup.rs
crates/codegen/xai-grok-shell/src/session/acp_session_impl/slash_exec.rs
crates/codegen/xai-grok-shell/src/session/acp_session_tests/inline_auto_compact_flow_tests.rs
crates/codegen/xai-grok-shell/src/session/compaction.rs
crates/codegen/xai-grok-shell/src/util/config/load.rs
crates/codegen/xai-grok-shell/src/util/config/mcp.rs
crates/codegen/xai-grok-shell/src/util/config/persist.rs
crates/codegen/xai-grok-shell/src/util/config/settings_writes.rs
docs/CONTROL_PLANE.md
docs/changes/2026-07-21-control-plane-implementation.md
out/goal-triad/completion_report.md
out/goal-triad/final-closeout-preflight.txt
out/goal-triad/final-completion-report.md
out/goal-triad/final-gap-matrix.md
out/goal-triad/final-verification-log.md
out/goal-triad/verification_log.md
```

## Repository handling

No file was staged or committed. Existing unrelated work was preserved. The detached workflow snapshot created during this task was inspected but not merged or modified.
