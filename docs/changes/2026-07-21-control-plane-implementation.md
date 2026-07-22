# Bandicot Control-Plane Implementation Ledger

Status: focused implementation verified; explicit gaps remain

Goal: implement Fast, Ultra, AgentGraph, Swarm, exact-100 mock benchmarking, durable recovery, resource controls, UI, ACP, headless operation, documentation, and tests as one coherent architecture.

| Phase | Status | Main files | Required verification |
| --- | --- | --- | --- |
| 0. Baseline and design alignment | verified | ADRs, this ledger, pipeline reports | preflight, baseline checks, secret scan |
| 1. Shared types and provider capabilities | verified | sampling types, sampler config, provider catalog | 284 type tests; provider wire suite |
| 2. Fast Mode | verified | sampler, shell, pager slash, config startup defaults | 14 wire tests plus persistence/config filters |
| 3. Ultra orchestration | verified | shell session state, subagent policy, pager | 8 focused Ultra tests |
| 4. AgentGraph core | verified | dependency-clean graph module | focused AgentGraph suite |
| 5. Durable graph store | verified primitives; automatic process-start recovery pending | SQLite graph store and artifacts | replay and stale-attempt tests |
| 6. Scheduler and local backend | verified implemented transitions | graph scheduler, resource manager, fake and subagent backends | resources, cancellation, pause/resume tests |
| 7. Verification and reduction | verified implemented checks | output schema and verification pipeline | unsafe path and plain-text rejection tests |
| 8. Graph UI, ACP, and headless | partial | slash commands, shell ACP resolver, pager passthrough, config gates | shell command tests; structured ACP/headless approval pending |
| 9. Swarm and exact-100 benchmark | fake path verified; live path unavailable | benchmark profile and fake role routing | peak-100 and capped-queue tests |
| 10. Bounded loops and compensation | pending | graph loop controller | bound and no-progress tests |
| 11. Optional Codex app-server backend | verified offline | JSON-RPC worker adapter | fake-server lifecycle test |
| 12. Hardening | partial | user guide, notices, changelog | targeted fmt/check/tests/secret scan; broad clippy pending |

## Recorded preflight

- Repository root: `/Users/miguel/Documents/bandicot`.
- Branch: `main`.
- Starting commit: `6e799e6`.
- Initial worktree: clean.
- `cargo metadata --no-deps --format-version 1`: passed.
- Root `Cargo.toml`: generated and must not be edited manually.
- Secret scan: three real-looking API-key-shaped fixtures found and replaced with explicit fake examples. Rotation is required if those committed values were ever real.

## Assumptions

- The attached specification is the complete goal contract.
- The local checkout is authoritative even where it differs from upstream or current Codex.
- No live provider swarm will be run during implementation or ordinary tests.

## 2026-07-22 closeout file-boundary ledger

Files outside `control_plane/agent_graph` were changed only where the runtime
boundary required it:

- `agent/config.rs` and `config_model_override_parse.rs`: add typed live-Swarm,
  worker-cap, retention, and prompt-cache capability configuration.
- `session/acp_session_impl/slash_exec.rs`: connect `/swarm run` to the existing
  real session worker backend while preserving root-only and configuration
  gates.
- sampler and sampling-types files: add capability-gated prompt-cache request
  emission and a separate low-level cache-write usage field.
- `docs/CONTROL_PLANE.md`: document the actual opt-in live path, approval
  binding, and current budget-reservation boundary.

These focused changes do not replace or discard the unrelated account and
context-compaction edits already present in the working tree.

## 2026-07-22 Safety Update

- Fake AgentGraph execution is restricted to explicit fake paths: `/swarm run --fake`, `/swarm benchmark --fake`, and focused unit tests.
- Real `/graph run` uses Bandicot's existing session subagent backend when it is
  available. Without that backend it reports execution as unavailable. It never
  substitutes fake worker completions.
- Active graph runs are attached in SQLite by `session_id` and repo root. The prior single cwd-level active-run pointer was removed to prevent concurrent sessions from overwriting each other.
- `/graph` and `/swarm` shell execution now pass the current ACP session id into the shared command service; subagent sessions remain blocked from those commands.
- `[orchestration]` config now provides startup defaults for Fast, hosted multi-agent, Ultra, Graph, and Swarm.
- Project `.grok/config.toml` can overlay orchestration defaults, but merged requirements are re-applied afterward so managed policy still wins.
- Session startup applies Fast and hosted multi-agent defaults against the selected provider/model capabilities before writing chat-state sampling config.
- `/fast` session commands persist only the requested Fast preference in `summary.json`; resume, fork, and provider/model changes recompute the effective service tier from current provider/model capabilities.
- Ultra startup can come from config when no persisted session override exists; session slash commands still win for restored non-default sessions.
- `/graph` and `/swarm` respect explicit `graph_enabled = false` and `swarm_enabled = false` config gates.
- Fast Responses API calls retry once without `service_tier` only when the provider returns a typed `service_tier` rejection.
- Settings modal controls for orchestration defaults are not complete; use config files or session slash commands for this stage.
- The optional Codex app-server AgentGraph worker adapter is disabled by default, uses read-only authority, and is tested through a fake JSON-RPC child process rather than a live Codex installation.
- The [control-plane user guide](../CONTROL_PLANE.md) documents the implemented commands, configuration and state behavior, GraphSpec v1, safety limits, recovery limits, ACP/headless status, and unavailable optional backends.
- Formal `smooth-software` is skipped for this stage because the working tree contains mixed uncommitted edits from multiple agents. Targeted polish only.
