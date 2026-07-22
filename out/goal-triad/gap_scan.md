# Gap Scan Report

Verdict: REQUEST CHANGES

Next Highest-Value Gap: Implement Phase 1 shared service-tier/provider capability types and the common Responses request-extension seam before Fast, Ultra, AgentGraph, or Swarm UI work.

## Evidence Read

- Goal contract: `/Users/miguel/.codex/attachments/27a8a857-af32-4137-bc21-345c476d8df5/pasted-text.txt`
- Stage contract: `.codex/pipelines/goal-triad/stages/01-gap-scan.toml`
- Implementation ledger: `docs/changes/2026-07-21-control-plane-implementation.md`
- Project instructions: no tracked `AGENTS.md`; `.agent/progress.md` and `.agent/plans.md` are absent.
- Source files inspected: `crates/codegen/xai-grok-sampler/src/config.rs`, `crates/codegen/xai-grok-sampling-types/src/conversation.rs`, `crates/codegen/xai-grok-sampler/src/client.rs`, `crates/codegen/xai-grok-pager/src/slash/commands/mod.rs`, `crates/codegen/xai-grok-shell/src/session/slash_commands.rs`, `crates/codegen/xai-grok-tools/src/implementations/grok_build/task/mod.rs`, `crates/codegen/xai-grok-tools/src/implementations/grok_build/task/types.rs`, `crates/codegen/xai-grok-shell/src/agent/subagent/handle_request.rs`, `crates/codegen/xai-codebase-graph/src/scope_graph/*`, `crates/codegen/xai-sqlite-journal/src/lib.rs`, `docs/decisions/README.md`, `docs/decisions/0004-agentgraph-control-plane.md`, `docs/decisions/0005-orchestration-and-service-tiers.md`, `docs/decisions/0006-worker-authority-and-resource-isolation.md`, `docs/changes/2026-07-18-control-plane-next.md`, `crates/codegen/xai-grok-sampler/tests/provider_wire.rs`.
- Commands run: `git status --short`, `git log -5 --oneline`, `git diff --stat`, `cargo metadata --no-deps --format-version 1`, `git diff --check`, targeted `git grep`/`rg` scans for Fast, Ultra, Swarm, AgentGraph, service-tier, provider-multi-agent, and exact-100 markers, plus a masked tracked-source secret-pattern scan.

## Missing Items

- Phase 1 is missing in source truth. The goal requires `ServiceTierPreference`, `EffectiveServiceTier`, `ResolvedServiceTier`, provider/model capability resolution, and a `SamplerConfig` service-tier field (`pasted-text.txt:491`, `pasted-text.txt:497`, `pasted-text.txt:503`, `pasted-text.txt:1644`). Current `SamplerConfig` has no service-tier or hosted multi-agent field (`crates/codegen/xai-grok-sampler/src/config.rs:102`).
- Fast Mode is not wired. The goal requires OpenAI Responses Fast to serialize `"service_tier":"priority"` (`pasted-text.txt:1623`) and to use one shared extension helper for streaming and non-streaming paths (`pasted-text.txt:1653`). Current typed conversion hardcodes `service_tier: None` (`crates/codegen/xai-grok-sampling-types/src/conversation.rs:2219`), and the two Responses send paths only call `finalize_response_body` without a provider-extension helper (`crates/codegen/xai-grok-sampler/src/client.rs:1525`, `crates/codegen/xai-grok-sampler/src/client.rs:1592`, `crates/codegen/xai-grok-sampler/src/client.rs:1733`).
- Fast, Ultra, Graph, and Swarm commands are absent. The goal requires `/fast`, `/ultra`, `/graph`, and `/swarm` commands (`pasted-text.txt:2524`). The pager builtin command list has no matching command modules or registrations (`crates/codegen/xai-grok-pager/src/slash/commands/mod.rs:76`), and shell builtin slash commands likewise do not include them (`crates/codegen/xai-grok-shell/src/session/slash_commands.rs:50`).
- Required ADRs are only partially clean. New ADRs now cover AgentGraph, orthogonal service tiers, and worker isolation (`docs/decisions/0004-agentgraph-control-plane.md:7`, `docs/decisions/0005-orchestration-and-service-tiers.md:11`, `docs/decisions/0006-worker-authority-and-resource-isolation.md:11`), but the ADR index now has two `0004` entries (`docs/decisions/README.md:9`, `docs/decisions/README.md:12`). That numbering collision should be fixed before treating the documentation slice as complete.
- AgentGraph core is missing. The goal requires a versioned declarative `GraphSpec`, node/edge/join/status types, validation, normalization, topology analysis, predicate evaluation, and scheduler-state transitions (`pasted-text.txt:466`, `pasted-text.txt:587`, `pasted-text.txt:778`). The only tracked `NodeKind`/`EdgeKind` matches are the existing scope graph for repository symbols (`crates/codegen/xai-codebase-graph/src/scope_graph/nodes.rs:138`, `crates/codegen/xai-codebase-graph/src/scope_graph/edges.rs:1`), which the goal explicitly says is not the orchestration graph (`pasted-text.txt:300`).
- Durable graph store and scheduler are missing. The goal requires SQLite graph tables, append-only events, leases, restart recovery, and a durable scheduler with ready-set calculation, resource acquisition, budgets, joins, loops, cancellation, and fairness (`pasted-text.txt:879`, `pasted-text.txt:969`, `pasted-text.txt:1007`, `pasted-text.txt:1123`). `xai-sqlite-journal` exists and can be reused (`crates/codegen/xai-sqlite-journal/src/lib.rs:21`), but there is no AgentGraph store or scheduler using it.
- Worker isolation is incomplete for the goal. Existing subagent depth is one (`crates/codegen/xai-grok-tools/src/implementations/grok_build/task/mod.rs:29`), and current child setup strips the task tool at max depth (`crates/codegen/xai-grok-shell/src/agent/subagent/handle_request.rs:406`). The goal requires graph workers to remove nested orchestration tools through explicit tool selection and not rely only on `MAX_SUBAGENT_DEPTH` (`pasted-text.txt:1443`, `pasted-text.txt:1455`).
- Exact-100 Swarm and safety gates are missing. The goal requires an exact-ready-agent benchmark, fake WorkerBackend proof, no live 100-agent tests by default, and explicit live opt-in (`pasted-text.txt:1967`, `pasted-text.txt:2108`, `pasted-text.txt:2122`). Targeted source search found no `BANDICOT_LIVE_SWARM`, `exact_ready_agents`, `abort_if_ready_agents_below`, or fake 100-worker benchmark markers.
- Required tests are not implemented or run. The goal requires Fast provider-wire tests, GraphSpec tests, scheduler tests, 100-worker mock benchmark tests, resource safety tests, UI tests, persistence/protocol tests, and security tests (`pasted-text.txt:3230`, `pasted-text.txt:3245`, `pasted-text.txt:3305`, `pasted-text.txt:3323`, `pasted-text.txt:3340`). Existing `provider_wire.rs` tests provider-boundary behavior, but it has no Fast `service_tier` assertions (`crates/codegen/xai-grok-sampler/tests/provider_wire.rs:37`).

## Why This Matters

The current repository is still before the first functional control-plane slice. Without the shared service-tier and provider-extension layer, Fast Mode cannot be proven on the wire, and later AgentGraph/Swarm node-level service-tier resolution would duplicate or bypass the sampler boundary.

## Suggested Next Step

Implement the smallest Phase 1 slice: add shared service-tier preference/effective/resolved types, provider/model capability fields for Fast and hosted multi-agent, an effective service-tier field on `SamplerConfig`, and a single `apply_provider_request_extensions` helper used by both `create_response` and `create_response_stream`. Add focused unit and provider-wire tests proving Standard, Inherit, unsupported provider, and Fast-to-`priority` behavior. Leave `/fast`, Ultra, AgentGraph, and Swarm UI for later slices.

## Non-Goals Preserved

- Do not run a live 100-agent provider test.
- Do not use `xai-codebase-graph` as the orchestration graph.
- Do not use `ScheduledTask` as the DAG representation.
- Do not enable recursive agent fan-out.
- Do not enable Fast automatically through Ultra or Swarm.
- Do not edit generated root `Cargo.toml` directly.

## Verification Notes

- `cargo metadata --no-deps --format-version 1`: passed, 79 packages.
- `git diff --check`: passed.
- Secret-pattern scan printed only masked values; matches inspected were fake examples or redaction canaries, not proof of live credentials.
- Full compile/test/clippy commands were not run because the required implementation slices are absent; running broad tests would not close the identified gap.
