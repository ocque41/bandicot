# Bandicot Control Plane Roadmap

Status: proposed

Owner: Bandicot fork

Proposed: 2026-07-18

## Problem And Outcome

The foundation work made Bandicot provider-neutral and host-controlled, but the
next architecture phase needs measurable local privacy, deterministic budgets,
recoverable structured context, and a supported subscription-backed worker
boundary.

The intended outcome is a sanitized personal engineering control plane that
preserves Grok Build's recovery machinery while using official Codex interfaces
as an optional high-level worker backend.

## Scope

- Record-and-replay evaluation and usage accounting.
- Compile-time removal of unwanted remote export/control surfaces.
- Fail-closed sandbox, egress, and writer worktree policy.
- A high-level `WorkerBackend` interface and Codex prototype.
- Structured result packets and deterministic admission control.
- Append-only context events and pinned-state projection.
- Projected compaction and deterministic repository intelligence.
- Event-driven watchers layered over the host scheduler.

## Non-Goals

- Calling undocumented ChatGPT endpoints.
- Pretending Codex is a raw chat-completion provider.
- Rewriting the compactor before baseline evaluation exists.
- Running large homogeneous agent swarms.
- Making the strongest model the default for every role.
- Replacing Rust, the TUI, ACP, or session persistence.

## Planned Sequence

| Phase | Status | Primary paths |
| --- | --- | --- |
| Evaluation baseline | proposed | new evaluation crate and fixtures |
| Privacy baseline | proposed | telemetry, remote settings, auth, update, plugin, and network policy paths |
| Worktree fail-closed | proposed | subagent/worktree lifecycle |
| Worker abstraction | proposed | new worker-backend crate and local adapter |
| Codex prototype | proposed | `codex exec`, then app-server adapter |
| Resource budgets | proposed | scheduler, usage ledger, worker lifecycle |
| Structured packets | proposed | worker protocol and app-server output schemas |
| Event ledger | proposed | append-only events, artifacts, checkpoints, projections |
| Projected compaction | proposed | compactor policy and coverage validation |
| Repository intelligence | proposed | symbol/build/test graph and bounded map |
| Event watchers | proposed | scheduler predicates and `/watch` surface |

## Engineering Rationale

Codex already owns an internal agent loop. Integrating it below Bandicot's
sampler would duplicate tools, permissions, repository exploration, and
compaction. A worker-level boundary keeps Bandicot authoritative for budgets,
worktrees, event history, handoffs, and cancellation while leaving the worker's
internal protocol intact.

Immutable events let model context become a reproducible projection instead of
the canonical state. Projected compaction then operates on measured future
growth and pinned invariants rather than a fixed transcript percentage.

## Security And Privacy

- Unknown egress is denied by default.
- Secret canaries and outbound-destination tests gate the privacy baseline.
- Remote settings cannot enable removed upload paths.
- Autonomous writers fail if isolation cannot be established.
- Credentials remain external references and never enter events or artifacts.
- Plugin and update sources require local allowlists and reviewed hashes.

## Acceptance Criteria

- No unwanted remote export path exists in the built personal profile.
- Network integration tests observe no unapproved destination.
- Every writer requiring isolation fails closed when worktree setup fails.
- Global and per-worker budgets are enforced deterministically.
- Worker results contain evidence and test status under a validated schema.
- Pinned constraints survive repeated compaction and replay.
- Context projections are reproducible from immutable source events.
- The Codex backend discovers model metadata and uses supported authentication.
- Event watchers invoke models only after configured state transitions.

## Verification Plan

- Golden replay fixtures for compaction and reconstruction.
- Secret-canary and denied-egress integration tests.
- Worktree failure injection.
- Worker budget and no-progress circuit-breaker tests.
- Structured-output rejection tests.
- Event projection determinism and checkpoint recovery tests.
- Fixed-threshold versus projected-compaction evaluation.
- Polling-loop versus event-watcher usage comparison.

## Review Checkpoints

1. Approve the privacy/evaluation baseline before worker integration.
2. Approve the worker protocol before implementing app-server transport.
3. Review event schemas before migrating authoritative context state.
4. Review replay evidence before changing compaction defaults.
5. Review watcher semantics before adding public slash commands.

## Research Basis

See
[Harness Architecture Review Intake](../research/2026-07-18-harness-architecture-review.md).
All external capability and benchmark claims require revalidation before use.
