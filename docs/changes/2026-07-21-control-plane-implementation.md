# Bandicot Control-Plane Implementation Ledger

Status: locally complete and focused-test verified as of 2026-07-22.

Bandicot now has one shared control-plane architecture for Fast, Ultra, AgentGraph, and Swarm:

- Fast resolves a requested service tier against live provider/model capabilities and sends the OpenAI Responses priority field only when supported.
- Ultra is root-only, depth one, and capped at six children.
- AgentGraph owns versioned specs, validation, preview, durable SQLite state, scheduling, budgets, leases, retries, loops, compensation, approvals, provider capacity, model selectors, artifacts, and recovery.
- Swarm uses AgentGraph with explicit live enablement, immutable approval, hard budgets, authority narrowing, model preflight, and adaptive canary concurrency.
- Slash commands, structured ACP methods, and headless clients use the same canonical graph service.
- Worker requests disable nested Bandicot orchestration and hosted provider multi-agent behavior.
- Prompt-cache keys are capability-gated and content-free; cached and cache-write usage are accounted separately.
- Settings modal controls persist typed orchestration defaults and preserve the distinction between user defaults and session overrides.

Local verification includes a 100-worker mock live-Swarm completion, an exact-100 peak-width fake benchmark, provider-wire priority-tier tests, startup recovery, budget enforcement, retry, loop, compensation, fairness, adaptive rate limiting, model resolution, ACP, approval, app-server fake lifecycle, and pager settings suites.

The only external-only check is an authenticated real `codex app-server`/provider account smoke. It is intentionally not automated because it depends on the user's installed account and must not start a paid turn or take ownership of credentials.

See [Control Plane](../CONTROL_PLANE.md) for user and operator behavior. Detailed local evidence is stored under `out/goal-triad/`.
