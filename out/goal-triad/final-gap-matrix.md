# Bandicot Control-Plane Final Gap Matrix

Date: 2026-07-22

Totals: **PASS 20 · FAIL 0 · EXTERNAL-ONLY 1**

| # | Capability | Result | Local evidence |
|---:|---|---|---|
| 1 | Fast Mode | PASS | Provider-wire suite proves supported Responses requests send `service_tier=priority`; unsupported providers omit it; typed rejection retries once without it. |
| 2 | Ultra Mode | PASS | Root-only policy, six-child ceiling, depth-one isolation, persistence, and status tests pass. |
| 3 | AgentGraph | PASS | Versioned graph schema, normalization, validation, topology, predicates, durable store, scheduler, worker contract, and canonical service pass focused tests. |
| 4 | Production Swarm execution | PASS | Opt-in live path uses the existing subagent backend, immutable approval, hard budget, model preflight, isolation, bounded concurrency, and durable status. Local mock completed 100/100. |
| 5 | Exact-100 Swarm benchmark | PASS | Fake benchmark reached peak 100 and completed 100/100; capped test queued 75 behind a limit of 25. |
| 6 | Runtime budgets | PASS | Durable reservation and lease are atomic; actual usage reconciles; missing usage charges conservatively; budget-stop test dispatches exactly 10 workers. |
| 7 | Startup recovery and lease expiry | PASS | Startup manager migrates, owns a coordinator lease, expires stale attempts, persists one retry, applies wall deadlines, and exposes restored runs. |
| 8 | Persisted retries | PASS | Typed failure classes, bounded deterministic jitter, persisted retry deadlines, Retry-After, and exactly-once activation pass. |
| 9 | Bounded graph loops | PASS | Persisted iteration state, deterministic generated IDs, token/call/time/node bounds, deduplication, and no-progress termination pass. |
| 10 | Compensation | PASS | Persisted saga plan runs completed side effects in reverse order and fails closed on required compensation failure. |
| 11 | Fair scheduling | PASS | Weighted deficit round robin, atomic multi-resource admission, head-of-line bypass, and interactive reserve tests pass. |
| 12 | Adaptive provider rate limiting | PASS | Typed request/token/project-token metadata crosses the child-session boundary; reservations, Retry-After, auth circuit, and adaptive Swarm ramp tests pass. |
| 13 | Model selectors | PASS | Live catalog resolution, exact Luna no-fallback, ordered Terra/Sol candidates, provider/capability/effort/tier constraints, and dispatch-time re-resolution pass. |
| 14 | Typed settings and settings modal | PASS | Config persistence, session overrides, modal definitions, action/effect/rollback arms, and settings tests pass. |
| 15 | Structured ACP graph APIs | PASS | All required versioned methods return structured DTOs through the canonical service and emit graph, node, approval, budget, and rate-limit notifications. |
| 16 | Headless approval | PASS | Approval binds normalized graph hash, revision, budget, effects, permissions, repository commit, and expiry; missing or changed approval fails closed. |
| 17 | Codex app-server worker support | EXTERNAL-ONLY | Local adapter and fake JSON-RPC lifecycle pass. A real `codex app-server` authenticated smoke depends on the installed user's external account/runtime and must not start a paid turn automatically. |
| 18 | Hosted OpenAI multi-agent isolation | PASS | Worker requests force hosted multi-agent off; nested orchestration tools are denied; provider-wire capability gates pass. |
| 19 | Prompt cache and usage accounting | PASS | Cache key is capability-gated and content-free; input, cached input, cache-write, output, reasoning, model/tool calls, and unknown cost are tracked separately. |
| 20 | TUI status, preview, and inspection | PASS | Real command capture shows Fast, Ultra, graph preview/status, Swarm status, approval, live mock start/final, and exact-100 output. Preview uses the canonical service. |
| 21 | Security, cleanup, telemetry, docs, E2E | PASS | Authority narrowing, cleanup/export, bounded status, masked secret scan, docs, compile, focused suites, and local mock E2E are complete. |

External-only manual check:

```sh
codex app-server
```

Using a JSON-RPC client, send `initialize`, `initialized`, `account/read`, and paginated `model/list`, then exit without `turn/start`. Success means valid JSON-RPC responses and clean process shutdown. Failure must leave AgentGraph unavailable for that backend without exposing or taking ownership of account credentials.
