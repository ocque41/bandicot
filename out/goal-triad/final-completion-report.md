# Bandicot control-plane closeout report

Date: 2026-07-22

## Outcome

This pass did not complete the full contract. It closed several concrete
runtime gaps without replacing the existing implementation, but the forensic
matrix still contains 21 local failures. The repository must not be described
as production-complete.

## Implemented in this pass

- Added a separate default-off `live_swarm_enabled` gate, worker-cap setting,
  and artifact-retention setting to typed orchestration configuration.
- Connected the root `/swarm run` path to the existing real session worker
  backend when the live gate is enabled. The fake path remains explicitly
  separate.
- Added hard-budget preflight for live Swarm.
- Added immutable execution approval binding across normalized graph hash,
  revision, budget, effects, permissions, repository commit, and expiry.
- Persisted execution approvals and added inspection/hash-acknowledgment
  commands.
- Added run budget reservations before scheduler dispatch, conservative charge
  behavior for missing usage, child usage propagation, and durable usage rows.
- Expanded budget dimensions and compensation terminal status vocabulary.
- Added built-in worker/reducer/verifier selector mapping into the existing
  child-session model validation path.
- Added capability-gated Responses `prompt_cache_key` emission derived without
  raw prompt or credential content.
- Added budget, approval, selector, and prompt-cache tests.

## Verification state

- Isolated `cargo check -p xai-grok-shell`: PASS.
- Focused AgentGraph suite initial run: 33 PASS, 1 FAIL because an existing
  fake-plan test used the old command parse order. The parser was corrected and
  the single test was rerun; see `final-verification-log.md` for its final exit.
- No paid provider call and no live 100-worker execution were performed.

## Files changed by this closeout pass

- `crates/codegen/xai-grok-shell/src/agent/config.rs`
- `crates/codegen/xai-grok-shell/src/agent/config_model_override_parse.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/approval.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/benchmark.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/budget.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/codex_app_server.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/commands.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/mod.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/scheduler.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/store.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/tests.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/types.rs`
- `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/worker.rs`
- `crates/codegen/xai-grok-shell/src/session/acp_session_impl/slash_exec.rs`
- `crates/codegen/xai-grok-sampler/src/client.rs`
- `crates/codegen/xai-grok-sampler/src/config.rs`
- `crates/codegen/xai-grok-sampler/tests/provider_wire.rs`
- `crates/codegen/xai-grok-sampling-types/src/types.rs`
- `docs/CONTROL_PLANE.md`
- `docs/changes/2026-07-21-control-plane-implementation.md`
- `out/goal-triad/completion_report.md`
- `out/goal-triad/final-closeout-preflight.txt`
- `out/goal-triad/final-completion-report.md`
- `out/goal-triad/final-gap-matrix.md`
- `out/goal-triad/final-verification-log.md`
- `out/goal-triad/verification_log.md`

This list is limited to this closeout pass. Other dirty worktree files existed
before the pass and were preserved.

## Remaining local failures

The exact blocking facts are in `final-gap-matrix.md`. The largest gaps are the
process runtime manager, persisted retry controller, loop execution,
compensation execution, host-wide fair admission, adaptive rate metadata,
catalog-backed selectors, typed settings UI, structured ACP, complete headless
approval transport, production Codex backend selection, beta collaboration
item parsing, full prompt-cache accounting, graph TUI, cleanup, and telemetry.

## Worktree safety

No reset, clean, restore, checkout, stash, staging, commit, push, paid provider
request, or unrelated Cargo-process termination was performed.
