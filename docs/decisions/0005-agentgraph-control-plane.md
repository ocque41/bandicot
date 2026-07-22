# 0005: AgentGraph Is the Host-Owned Orchestration Control Plane

Status: accepted

## Context

Bandicot needs small proactive teams and explicitly authorized high-width execution without giving scheduling authority to a model. The earlier control-plane roadmap listed large homogeneous swarms as a non-goal. This decision supersedes that statement only for validated, budgeted AgentGraph runs and the controlled exact-100 benchmark.

## Decision

AgentGraph is a versioned, declarative graph specification and a deterministic host runtime. Models may propose a graph, but the host parses, normalizes, validates, persists, approves, schedules, resumes, cancels, and verifies it. Model-generated JavaScript, imports, evaluators, shell control logic, arbitrary file access, and arbitrary sockets are forbidden in the control plane.

Graph validation is deterministic so the same normalized revision produces the same topology, limits, content hash, and validation result. A running graph binds to one immutable normalized revision. Edits create a new revision.

Total graph size and active concurrency are separate limits. Production Swarm has configurable capacity up to 100 active model workers but no minimum. Exact readiness of 100 independent workers is required only by the benchmark profile. General recursive model-agent trees remain disabled and model-agent depth remains one; later reducers, verifiers, and synthesis are host-scheduled phases rather than grandchildren.

High-cost execution requires a host-built budget preview and explicit approval. Headless execution requires an explicit acknowledgment and hard budgets. Graph state is append-only and durable: every meaningful transition is recorded as an event and transactionally reflected in materialized SQLite state using the filesystem-aware journal selection.

## Consequences

- Graph cardinality never implies dispatch concurrency.
- General cycles are rejected in GraphSpec v1; iteration uses bounded Loop contracts.
- Failures remain typed data through reducers and final status.
- Repository-provided graphs never auto-run.
- The exact-100 profile can prove scheduler behavior with a fake backend without making live provider requests.
