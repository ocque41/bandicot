# Progress

Updated: 2026-07-22 05:17:28 CEST

## Completed

- Read active goal, gap scan, strategy report, stage config, implementation ledger, ADRs, roadmap, research intake, README, and fork notice.
- Ran mandatory preflight commands:
  - `git rev-parse --show-toplevel`
  - `git status --short`
  - `git branch --show-current`
  - `git log -5 --oneline`
  - `git diff --stat`
  - `git diff --name-only`
  - `cargo metadata --no-deps --format-version 1`
- Confirmed there is no tracked `AGENTS.md`.
- Spawned `provider_fast` for service-tier/Fast/provider wire work.
- Spawned `agentgraph_core` for GraphSpec/core validation work.
- Confirmed provider wire tests passed in the root validation session:
  - `provider_wire` 8 tests passed, including Fast streaming/non-streaming parity.
- Confirmed AgentGraph core validation now covers headers, topology, bounded maps/loops/retries, authority, worker isolation, references, write conflicts, and exact-ready counts.
- Added/verified by static review that AgentGraph command execution no longer uses `FakeWorkerBackend` for real `/graph run`.
- Gated fake execution to explicit fake paths: `/swarm run --fake`, `/swarm benchmark --fake`, and AgentGraph unit tests.
- Replaced the global `.agent/agentgraph-active-run` pointer with a durable SQLite active-run attachment keyed by `session_id` and repo root.
- Wired `/graph` and `/swarm` shell execution to pass the ACP session id into the shared command service.
- Updated command tests to prove no fake completion is reported for `/graph run`, `/swarm run` without `--fake` is unavailable, and separate sessions do not share active runs.
- Added a typed `[orchestration]` config section for startup defaults:
  - `fast_service_tier`
  - `hosted_multi_agent`
  - `hosted_multi_agent_max_concurrent_subagents`
  - `ultra_enabled`
  - `ultra_max_children`
  - `graph_enabled`
  - `swarm_enabled`
  - `disable_provider_multi_agent_for_workers`
- Wired session startup to apply `[orchestration]` Fast, hosted multi-agent, and Ultra defaults against the selected model/provider capabilities.
- Wired `/graph` and `/swarm` entry points to respect `graph_enabled = false` and `swarm_enabled = false`.
- Preserved requirements/managed-policy precedence over project `.grok/config.toml` orchestration overrides by re-applying requirements after project overlay.
- Added Fast fallback behavior for typed OpenAI Responses `service_tier` rejection:
  - non-streaming and streaming retry once without `service_tier`
  - generic validation errors and auth errors do not retry
  - all other request fields are preserved
- Added durable `/fast` session preference persistence:
  - summary storage persists only requested `fast` or `standard`
  - resume/fork recomputes effective provider support from current capabilities
  - model/provider changes preserve the request and recompute the effective tier

## In Progress

- Focused Cargo verification is queued/running in other active agents. Per root instruction, no additional Cargo command is being started from this agent until that queue clears.
- Settings modal coverage for orchestration defaults is not implemented. The current safe path is config-file startup defaults plus session slash commands.

## Source Stability

- Fast persistence source, docs, and reports are reconciled and stable for root-captured validation.
- `rustfmt --edition 2024` passed on the touched Rust files.
- `git diff --check` passed.
- A focused Fast Cargo test was started after the default target cleared, but it was blocked on the build lock and was stopped when root clarified that this agent should not start Cargo during external context/accounts tests.

## Pending

- Confirm queued focused checks:
  - `cargo test -p xai-grok-shell control_plane::agent_graph --lib`
  - sampler/provider wire checks covering Fast request extensions
  - Ultra runtime focused checks from the Ultra owner
- Run targeted `rustfmt`/`git diff --check` after external Cargo jobs finish.
- Root-captured validation can begin after external context/accounts tests clear.
