# Architecture Solidification Ideas

Source truth read: active goal attachment, `out/goal-triad/gap_scan.md`, `docs/changes/2026-07-21-control-plane-implementation.md`, `.codex/pipelines/goal-triad/stages/02-strategist-solidify.toml`, ADRs `0002`, `0003`, `0005`, `0006`, `0007`, and targeted source. No tracked `AGENTS.md`, `GOAL.md`, `.agent/progress.md`, or `.agent/plans.md` were present. Assumption: the prompt-provided AGENTS instructions are the active project instructions.

1. Make Phase 1 a hard shared contract before UI or graph work.
   Evidence: the goal requires `ServiceTierPreference`, `EffectiveServiceTier`, `ResolvedServiceTier`, provider/model capability resolution, and an effective `SamplerConfig` field. Current `SamplerConfig` has provider capabilities but no service-tier field (`crates/codegen/xai-grok-sampler/src/config.rs:45`, `crates/codegen/xai-grok-sampler/src/config.rs:102`). Target: define service-tier primitives in a leaf shared module such as `crates/codegen/xai-grok-sampling-types/src/service_tier.rs`, export them, then thread them through `SamplingConfig`, `SamplerConfig`, `ModelInfo`, `ConfigModelOverride`, `sampling_config_for_model`, and `reconstruct_full_config`. Tests: config resolver unit tests plus provider-wire tests for Inherit, explicit Standard, Fast-supported, and unsupported-provider resolution.

2. Put all Responses request body extensions behind one sampler helper.
   Evidence: typed Responses conversion hardcodes `service_tier: None` (`crates/codegen/xai-grok-sampling-types/src/conversation.rs:2219`). Streaming and non-streaming paths both call `finalize_response_body`, but there is no shared provider-extension helper (`crates/codegen/xai-grok-sampler/src/client.rs:1525`, `crates/codegen/xai-grok-sampler/src/client.rs:1592`, `crates/codegen/xai-grok-sampler/src/client.rs:1733`). Target: evolve `finalize_response_body` or add `apply_provider_request_extensions` in `xai-grok-sampler/src/client.rs`. It should add `"service_tier":"priority"` only for supported Responses requests, keep xAI-only extensions capability-gated, and be used by both send paths. Tests: exact request JSON assertions in `crates/codegen/xai-grok-sampler/tests/provider_wire.rs`.

3. Persist requested service-tier intent separately from effective transport state.
   Evidence: ADR `0006` says explicit Standard is distinct from Inherit and model/provider changes recompute effective support without discarding requested intent. `SamplingConfig` currently carries model, backend, headers, context, reasoning effort, and stream tool calls, but no service-tier intent (`crates/codegen/xai-grok-sampling-types/src/types.rs:1060`). Target: keep requested preference in session/config state and only put the resolved effective value on `SamplerConfig`. Tests: model switch preserves requested Fast/Standard intent while recomputing effective support.

4. Build AgentGraph as a dependency-clean host module after Phase 1.
   Evidence: ADR `0005` requires a host-owned declarative graph; the gap scan found only the repository scope graph, not an orchestration graph. Target: use an internal module such as `crates/codegen/xai-grok-shell/src/control_plane/agent_graph/` unless a sanctioned workspace generator path is identified for a new crate. Keep schema, normalization, validation, topology, predicate evaluation, and status transitions separate from pager rendering and subagent spawning. Tests: JSON/YAML round trips, canonical hash stability, duplicate-node rejection, cycle rejection, join validation, and budget/resource validation.

5. Treat durable graph state as event-sourced SQLite state, not session transcript state.
   Evidence: ADR `0005` requires append-only durable graph transitions; `xai-sqlite-journal` already provides filesystem-aware SQLite opening (`crates/codegen/xai-sqlite-journal/src/lib.rs:21`). Target: graph store tables for graph revisions, runs, node attempts, resource leases, artifacts, and append-only events. Use materialized state only as a replay cache. Tests: migration bootstrap, replay from events, restart recovery, stale lease handling, cancellation replay, and artifact lookup.

6. Make worker authority and resources explicit before Ultra or Swarm execution.
   Evidence: existing subagent depth is one (`crates/codegen/xai-grok-tools/src/implementations/grok_build/task/mod.rs:29`) and `handle_request` strips the Task tool only at max depth (`crates/codegen/xai-grok-shell/src/agent/subagent/handle_request.rs:405`). ADR `0007` requires capability ceilings, tool allow/deny lists, environment allowlists, write sets, and durable resource claims. Target: add a graph-worker launch policy that removes nested orchestration tools and provider-hosted multi-agent independently of depth. Tests: graph workers cannot receive Task/graph/swarm tools, Plan Mode rejects write-capable graph nodes, and resource permits serialize builds, writers, ports, and external APIs.

7. Keep slash commands thin and state-backed.
   Evidence: required `/fast`, `/ultra`, `/graph`, and `/swarm` commands are absent from pager and shell builtin lists (`crates/codegen/xai-grok-pager/src/slash/commands/mod.rs:76`, `crates/codegen/xai-grok-shell/src/session/slash_commands.rs:50`). Target: add commands only after the underlying state/resolution/runtime exists. Commands should call shared state transitions and render requested/effective status, not duplicate validation logic. Tests: command parsing/status tests plus ACP command exposure tests.

8. Prove exact-100 Swarm with a fake backend only.
   Evidence: the goal requires exact-ready-agent benchmark behavior and live opt-in markers, but no `BANDICOT_LIVE_SWARM` or `exact_ready_agents` source markers exist. Target: add a fake `WorkerBackend` benchmark profile after scheduler/resource control. Tests: peak ready count equals 100, no live provider calls by default, live mode requires explicit environment and budget acknowledgment.

# Reliability Blockers

- Phase 1 service-tier and provider capability contracts are missing. This blocks Fast Mode and later per-node Swarm service-tier resolution.
- The Responses wire path cannot yet prove Fast Mode because `service_tier` is always absent.
- AgentGraph core is missing. Ultra and Swarm must not be built as prompt-only behavior.
- Durable graph store and scheduler are missing. Swarm, recovery, cancellation, resource leases, and exact-100 proof depend on this.
- Worker isolation currently depends too much on subagent depth. Graph workers need explicit authority and resource policy.

# Foundation Improvements

- Add a service-tier resolver test matrix in `crates/codegen/xai-grok-shell/src/agent/config.rs` tests and sampler provider-wire tests in `crates/codegen/xai-grok-sampler/tests/provider_wire.rs`.
- Add golden request JSON fixtures for Responses streaming and non-streaming parity.
- Add GraphSpec schema fixtures under the future graph module tests: minimal graph, fan-out/fan-in graph, invalid cycle, invalid join, unsupported provider capability, and exact-100 benchmark graph.
- Add graph event replay tests before any UI work. The UI should consume durable state snapshots, not infer run state from scrollback.
- Keep ADR `0006` and `0007` as acceptance checks for every implementation slice: Fast must not imply Ultra/Swarm, and Ultra/Swarm must not weaken permissions.

# Tradeoffs

- Putting service-tier primitives in `xai-grok-sampling-types` avoids a new crate and avoids editing generated root `Cargo.toml`. Cost: the type module becomes part of the sampling public surface.
- Extending the existing sampler `ProviderCapabilities` is the lowest-churn path. Risk: graph-core code may later want provider facts without depending on the sampler. Mitigation: keep graph specs storing data-only capability requirements, then resolve them at shell runtime.
- Starting AgentGraph inside `xai-grok-shell` avoids workspace generator risk. Cost: it is not as clean as a new crate. This is acceptable for the milestone if the module has narrow dependencies and strong tests.
- Delaying UI commands costs visible progress, but it prevents command behavior from hardcoding policy before the contract and runtime are proven.

# Suggested Ordering

1. Implement Phase 1 service-tier types, provider capability fields, resolver, `SamplerConfig` effective field, and provider-wire tests.
2. Implement `/fast on|off|status` against requested/effective state.
3. Implement AgentGraph schema, normalization, validation, topology, and status types.
4. Implement durable graph store and replay using `xai-sqlite-journal`.
5. Implement scheduler, fake worker backend, resource permits, authority filtering, and cancellation.
6. Implement Ultra as root-only graph-backed proactive delegation.
7. Implement `/graph`, ACP/headless graph operations, and dashboard state views.
8. Implement Swarm and exact-100 fake benchmark profile.
9. Run hardening: fmt, targeted tests, clippy for touched crates, secret scan, docs, and notices.

# Out Of Scope

- No live 100-worker provider test in ordinary verification.
- Do not use `xai-codebase-graph` as the orchestration graph.
- Do not use `ScheduledTask` as the DAG representation.
- Do not enable recursive model-agent fan-out.
- Do not make Fast automatic through Ultra or Swarm.
- Do not edit generated root `Cargo.toml` directly.
- Do not implement provider-hosted multi-agent as a replacement for AgentGraph.
