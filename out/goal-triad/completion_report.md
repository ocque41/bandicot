# Sure Polish Completion Report

> Superseded for the 2026-07-22 closeout attempt by
> `final-completion-report.md` and `final-gap-matrix.md`. The closeout remains
> incomplete and records 21 local FAIL rows; do not use the earlier summary as
> evidence of production completion.

Updated: 2026-07-22 05:48:50 CEST

Status: implemented and focused-test stable, with the explicit incomplete
items listed below.

## What Changed

- Added shared service-tier and hosted multi-agent capability plumbing for sampler/provider request construction.
- Added typed `[orchestration]` startup defaults for Fast, hosted multi-agent, Ultra, Graph, and Swarm.
- Applied orchestration startup defaults in session startup with model/provider capability checks.
- Added project `.grok/config.toml` orchestration overlay while re-applying requirements so managed policy remains higher priority.
- Added `/graph` and `/swarm` config gates for `graph_enabled = false` and `swarm_enabled = false`.
- Added conservative one-time retry for typed OpenAI Responses `service_tier` rejection in both streaming and non-streaming Fast paths.
- Added durable `/fast` requested preference persistence in session summaries; resume, fork, and provider/model changes recompute effective service-tier support instead of trusting stale effective state.
- Added `/fast`, `/ultra`, `/graph`, and `/swarm` command surfaces across shell and pager layers.
- Added AgentGraph core modules for schema, normalization, validation, topology, predicate evaluation, resources, scheduling, durable SQLite store, verification, fake worker backend, and exact-100 fake benchmark.
- Added command-level AgentGraph and Swarm control behavior:
  - `/graph validate` and `/graph preview` validate and show topology, budgets, model selectors, permissions, effects, and resources.
  - `/graph plan` stores an awaiting-approval run scoped to the current session.
  - `/graph run` never substitutes fake workers. When the session subagent
    backend is available, it runs asynchronously through that backend; when it
    is unavailable, it reports the limitation and records no fake completion.
  - `/swarm run --fake` and `/swarm benchmark --fake` are the only command paths that run the fake backend.
  - active runs are attached in SQLite by `session_id` and repo root, not by a single cwd-level pointer.
- Added Ultra session persistence and root-only policy injection work owned by the Ultra runtime slice.

## Success Criteria Map

| Requirement | Current Evidence | Status |
| --- | --- | --- |
| Fast service tier serializes OpenAI `service_tier: "priority"` only when supported | Final provider-wire run: 14/14 passed | implemented and verified |
| Fast service tier retries once without `service_tier` on typed provider rejection only | Streaming and non-streaming provider-wire cases | implemented and verified |
| Fast requested preference persists through resume/fork without persisting stale effective support | 4 Fast storage/restore tests plus summary round trip | implemented and verified |
| Orchestration config resolves model/provider capability and startup defaults | 4 focused config tests | implemented and verified for represented layers |
| Ultra is root-only, persisted, and policy-injected without recursive fan-out | 8 focused Ultra tests | implemented and verified |
| AgentGraph has typed schema, validation, normalization, topology, predicates | AgentGraph shell suite | implemented and verified |
| Durable graph store records specs/runs/nodes/events/leases/artifacts | replay/stale-attempt/session-scope tests | implemented and verified |
| Scheduler handles dependencies, resources, joins, pause/drain/resume/cancel | AgentGraph scheduler suite | implemented and verified for implemented transitions |
| Fake exact-100 benchmark proves 100 configured workers without live provider run | exact width and capped queueing tests | implemented and verified |
| Real graph commands do not fake completion | real-backend and fake-path separation tests/static review | implemented and verified |
| Active runs are session-scoped | session-scoped store command test | implemented and verified |
| Settings UI exposes orchestration defaults | Existing settings modal lacks orchestration live state/reset/rollback/write plumbing | missing |
| Docs and stage artifacts are current | Ledger, progress, completion report, verification log, manifest updated | implemented |

## Limits

- Real graph execution uses the existing session subagent backend. Concrete
  model-selector resolution is not available yet, so named selectors are
  rejected rather than being sent as if they were subagent role names.
- Settings modal controls for orchestration defaults are not complete. Use config files or session slash commands for now.
- `disable_provider_multi_agent_for_workers` is parsed as orchestration policy input, but user-supplied GraphSpec files remain the source of truth for worker execution policy.
- Formal `smooth-software` is skipped because the working tree contains mixed uncommitted changes from multiple agents. Targeted polish was applied instead.
- Graph-wide cost/token/wall-time hard enforcement, loops, retries,
  compensation, scheduler fairness/rate-limit adaptation, and automatic
  process-start recovery are not complete end to end.
- Structured AgentGraph ACP methods, a headless approval protocol, and settings
  modal controls are not complete. The textual slash-command ACP surface is
  present.
- Broad clippy and pager command-filter validation remain unexecuted because a
  concurrent task held Cargo's default build lock with a large pager build.
