# Bandicot Control-Plane Final Verification Log

Date: 2026-07-22

## Passed commands

- `cargo fmt --all -- --check` — PASS after final formatting.
- `cargo check -p xai-grok-shell -p xai-grok-pager -p xai-grok-sampler -p xai-grok-sampling-types --target-dir /private/tmp/bandicot-agentgraph-target` — PASS.
- `cargo test -p xai-grok-shell control_plane::agent_graph --lib ...` — PASS: 56 passed, 0 failed.
- Diagnostic command/mock-Swarm capture — PASS: 1 passed, 0 failed.
- `cargo test -p xai-grok-shell extensions::agent_graph --lib ...` — PASS: 2 passed, 0 failed.
- `cargo test -p xai-grok-sampler --test provider_wire ...` — PASS: 15 passed, 0 failed.
- `cargo test -p xai-grok-sampler rate_limit ...` — PASS: 6 passed, 0 failed.
- `cargo test -p xai-grok-pager settings --lib ...` — PASS: 321 passed, 0 failed, 1 ignored visual smoke.
- `git diff --check` — PASS.
- Staged-file count — PASS: 0.

## Captured functional signals

- Fast: requested Fast; effective wire value `priority`; supported; session source.
- Ultra: requested/effective on; session source; maximum children 4; depth 1.
- Graph preview: revision 1; 100 static nodes; ready width 100; theoretical width 100; concurrency 100; model calls 100; input range 0..2,000,000; output range 0..500,000; cost Unknown.
- Local mock live Swarm: started with subagent backend, then Completed with 100 succeeded, 0 failed, and 100 model/tool/attempt charges.
- Exact-100 fake benchmark: workers 100; peak 100; completed 100; failures/timeouts/cancellations 0.
- Provider wire: supported streaming and non-streaming request bodies contain `"service_tier":"priority"`.

## Focused behavior covered by the 56 AgentGraph tests

- automatic recovery and downtime wall deadline;
- atomic durable budget admission and conservative missing usage;
- persisted retry deadline and exactly-once readiness;
- bounded loop and no-progress termination;
- reverse-order compensation;
- fair multi-resource admission and interactive reserve;
- adaptive provider capacity and auth circuit;
- live model-selector resolution;
- Codex app-server fake lifecycle;
- real subagent scheduling, pause/resume/cancel, output verification, and provider metadata crossing the child-session boundary.

## Failed commands retained for honesty

- The first diagnostic capture failed because its temporary repository had no `.git/HEAD`; the test fixture was corrected with a deterministic fake commit and then passed.
- Workspace strict clippy reached unrelated existing lints in `xai-grok-tools/src/types/fallback_provider.rs` (`derivable_impls`, `collapsible_if`).
- Package-only no-dependency strict clippy reached existing sampler lints in `apple.rs` and `client.rs` (`result_large_err`, `unnecessary_filter_map`).
- These clippy findings are outside the control-plane change and remain unresolved; compile, formatting, and focused tests pass.

## Secret scan

No secret values were printed. A masked high-entropy pattern scan identified three unchanged test/fixture files. None is modified by this implementation. Generic credential-word matches occur throughout typed configuration and tests and are not evidence of embedded credentials.

## Repository state

- Branch: `main` at starting commit `31c102d`.
- No files staged.
- No commit created.
- A later detached workflow-integration snapshot at `/private/tmp/bandicot-workflow-integration-019f8ab3` was inspected read-only. It contains broad unrelated work and no separate committed AgentGraph implementation to merge.
