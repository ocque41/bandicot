# Plan

1. Preserve pre-existing worktree changes and run mandatory preflight.
2. Resolve Phase 1 service-tier/provider capability blocker and Fast wire tests.
3. Implement dependency-clean AgentGraph core types, normalization, validation, and topology.
4. Implement durable SQLite store, event replay, scheduler/resource controls, worker backend interfaces, and fake exact-100 benchmark.
5. Add graph/Swarm/Ultra/Fast command surfaces and status outputs without live 100-agent behavior.
6. Add documentation and update the implementation ledger.
7. Run targeted fmt/check/tests/clippy/diff and secret scan.
8. Write completion report, verification log, and stage manifest.

## Delegation

- `provider_fast`: owns sampler/Fast provider wire slice.
- `agentgraph_core`: owns typed AgentGraph core validation slice.
- Parent/root implementation: owns integration, store, scheduler, commands, docs, reports, and final verification.

