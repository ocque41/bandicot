# Bandicot final closeout gap matrix

Date: 2026-07-22

This matrix applies the requested rule: a local row is `PASS` only when
production code is connected to the runtime, persistence is complete where
required, meaningful tests exist, and user-facing behavior is documented.

| Acceptance criterion | Status | Evidence or blocking fact |
| --- | --- | --- |
| Fast service-tier request, fallback, persistence, and wire coverage | PASS | Priority wire serialization and typed one-time Standard fallback are connected and covered by provider-wire tests. |
| Ultra root policy, depth-one isolation, restore, and fork state | PASS | Root/session policy and child restrictions are connected and covered by focused tests. |
| Exact-100 offline scheduler benchmark | PASS | Fake backend creates exactly 100 worker nodes and covers the 100 and 25 active-worker caps. |
| AgentGraph production execution as one durable control plane | FAIL | Command-spawned scheduling exists, but there is no process-owned runtime manager and several node semantics remain schemas only. |
| Provider-backed live Swarm | FAIL | The opt-in real backend path, hard-budget preflight, and bound approval now exist, but catalog preflight, canary ramping, and local-mock production-path E2E coverage are absent. |
| Runtime budget enforcement | FAIL | Scheduler reservations prevent in-process oversubscription and child usage is recorded, but reservation-plus-lease is not one durable transaction and wall/cost/percentage enforcement is incomplete. |
| Automatic startup recovery and lease expiry | FAIL | Store replay and lease expiry primitives exist; process-start ownership, heartbeat, automatic timers, and backend reconciliation do not. |
| Persisted retries | FAIL | Retry policy types exist, but chosen retry deadlines/classifications are not durable graph state. |
| Bounded graph loops | FAIL | Loop bounds validate, but the scheduler does not execute loop iterations or persist loop controller state. |
| Compensation | FAIL | Compensation types exist, but saga ordering, durable execution, retry, and terminal compensation states are not connected. |
| Fair scheduling and interactive reservation | FAIL | Per-run resource acquisition is atomic; host-wide fair admission and interactive reserve are absent. |
| Adaptive provider rate limiting | FAIL | `Retry-After` reaches request retry logic; typed request/token/project headers and scheduler capacity adaptation are absent. |
| Real model-selector catalog resolution | FAIL | Built-in selectors now reach child model validation, but validation/lease-time resolution against ModelsManager and app-server `model/list` is absent. |
| Typed orchestration settings modal integration | FAIL | Canonical config fields include live Swarm, worker cap, and retention, but pager metadata/actions/persistence/rollback/live apply are absent. |
| Structured ACP graph APIs and notifications | FAIL | Graph behavior remains textual slash-command output. |
| Non-interactive headless approval | FAIL | Immutable binding, expiry, persistence, and hash acknowledgment exist, but a structured ACP/CLI approval object and full headless E2E are absent. |
| Codex app-server production worker support | FAIL | The adapter handles core lifecycle calls against a fake process, but production selection, model/account preflight, approval bridging, usage conversion, and reconnect are absent. |
| Hosted OpenAI multi-agent capability isolation | FAIL | Default-off request gating and local-worker isolation exist; beta collaboration output items and status/warning behavior are absent. |
| Prompt-cache integration and accounting | FAIL | Capability-gated `prompt_cache_key` emission and cached-read parsing exist; stable graph prefix/sharding, cache-write propagation, and wire/accounting E2E are absent. |
| TUI status, inspection, and aggregated Swarm rendering | FAIL | Slash commands pass through; effective indicators, graph views, inspection, and bounded aggregation are absent. |
| Graph preview and linter completion | FAIL | Hash/topology/budget/effect values are real; requested estimates, selector resolution, and actionable lint warning set are absent. |
| Worker authority and security completion | FAIL | Read-only and nested-orchestration restrictions exist; environment/tool/network enforcement, symlink containment, size limits, and trust/approval E2E are absent. |
| Artifact retention and cleanup | FAIL | Retention config is typed, but cleanup execution, dry run, safety references, and tests are absent. |
| Control-plane telemetry and privacy tests | FAIL | Durable basic graph events exist; the requested event catalog and redaction assertions are absent. |
| Documentation and complete end-to-end/failure-injection matrix | FAIL | The guide reflects the new live gate, approval, and reservation behavior; the complete requested docs and E2E/failure suite are absent. |
| Paid real-provider exact-100 execution | EXTERNAL-ONLY | Local fake exact-100 coverage exists. Manual check requires an explicitly funded account and must use the opt-in live gate and a bound approval. |
| Exact account/proxy concurrency and rate-header behavior | EXTERNAL-ONLY | Local mock coverage is required before this external check; the current local adaptive controller is not complete, so this row does not convert the local gap into an external limitation. |
| Account-specific priority-tier entitlement | EXTERNAL-ONLY | Capability detection and typed fallback exist; verify with `/fast status` and one explicitly authorized provider request. Rejection must fall back once to Standard. |
| Provider-controlled hosted multi-agent beta behavior | EXTERNAL-ONLY | Request gating is mock-tested. A live beta response must surface collaboration items without executing them as developer functions; local parsing is still a FAIL above. |
| Live Codex executable smoke | EXTERNAL-ONLY | Fake process coverage exists. Run `codex app-server`, initialize once, send `initialized`, then call `account/read` and `model/list` without starting a paid turn. Missing executable/authentication must be reported clearly. |

## Totals

- PASS: 3
- FAIL: 21
- EXTERNAL-ONLY: 5
