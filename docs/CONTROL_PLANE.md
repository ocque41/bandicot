# Fast, Ultra, AgentGraph, and Swarm

This guide describes the control-plane behavior available in the current
Bandicot build. Fast, Ultra, AgentGraph, and Swarm are separate controls. One
does not silently enable another.

## Availability at a Glance

| Feature | Current status |
| --- | --- |
| Fast | Available for the current session and as a startup default |
| Ultra | Available for user-facing root sessions and persisted with the session |
| AgentGraph validation and preview | Available for JSON and YAML GraphSpec v1 files |
| AgentGraph execution | Available through the session subagent backend with read-only or isolated-worktree workers |
| Swarm exact-100 benchmark | Available offline with the fake backend |
| Live Swarm execution | Available through the session worker backend behind a separate opt-in gate and bound approval |
| Automatic graph restart after a process crash | Startup recovery scans nonterminal runs, expires stale leases, restores persisted retries, and enforces wall deadlines |
| Codex app-server worker backend | Available as an optional adapter; disabled by default |

All four slash commands are session commands. They are advertised to ACP
clients and are handled by the shell when submitted as slash-command prompts.
Child sessions cannot use Ultra, AgentGraph, or Swarm.

## Fast

Fast is a provider service-tier preference. It does not change the model,
reasoning effort, permissions, sandbox, approval policy, or orchestration mode.

```text
/fast on
/fast off
/fast status
```

When the selected provider and model advertise OpenAI Responses priority-tier
support, `/fast on` sends `service_tier: "priority"`. If support is not
advertised, Bandicot keeps the Fast request visible in status, uses the
standard tier, and explains why priority was not used.

If a supported request receives a typed rejection specifically for
`service_tier`, Bandicot retries that request once without the field. Generic
validation and authentication failures are not retried by this fallback.

`/fast off` explicitly selects the standard tier. `/fast status` shows the
requested value, effective wire value, support result, and source.

The slash command persists only the requested session preference. A restored
session applies that request before configuration defaults, then recomputes the
effective support result from the currently selected provider and model. Forked
sessions inherit the requested Fast preference and perform the same recompute.
Bandicot does not treat a previously persisted effective priority result as
authoritative.

## Ultra

Ultra lets a user-facing root session proactively delegate independent work to
a small number of child agents.

```text
/ultra on
/ultra on --max-children 4
/ultra off
/ultra status
```

The child limit is clamped to the range 1 through 6. Delegation depth is one:
Ultra children cannot spawn more agents or invoke Ultra, AgentGraph, Swarm, or
provider-hosted multi-agent features. Ultra does not enable Fast, YOLO,
always-approve behavior, broader tools, or weaker isolation.

The command updates and persists the root session's Ultra state. The policy is
applied once at the start of the next user turn. Disabling Ultra does not cancel
children that are already running. A restored root session uses its persisted
non-default Ultra setting before the configuration default. Child sessions are
always forced to Ultra off.

`/ultra status` reports requested and effective state, setting source, child
limit, active and pending child counts, Fast status, and the graph run attached
to the session.

## Configuration

Place startup defaults in the user configuration or a project
`.grok/config.toml` file:

```toml
[orchestration]
fast_service_tier = "standard" # "inherit", "standard", or "fast"
ultra_enabled = false
ultra_max_children = 4
graph_enabled = true
swarm_enabled = false
live_swarm_enabled = false
swarm_max_active_model_workers = 25
graph_artifact_retention_days = 30

[orchestration.codex_app_server]
enabled = false
executable = "codex"
args = ["app-server"]
```

The effective precedence is managed requirements, the nearest project
configuration, user configuration, managed configuration, system-managed
configuration, then built-in defaults. Managed requirements are reapplied
after the project overlay.

`fast_service_tier` is resolved against the selected provider and model at
session startup when the session has no persisted `/fast` request. Persisted
Fast requests are recomputed on resume, fork, and provider/model changes.
`ultra_enabled` and `ultra_max_children` provide the initial Ultra state when
the session has no persisted non-default override.
`graph_enabled = false` and `swarm_enabled = false` block their respective
command surfaces. Setting either value to `true` enables the command surface;
it does not add a missing execution backend.

The settings interface exposes Fast startup tier, Ultra defaults, Graph and
Swarm gates, the separate live-Swarm gate, the Swarm worker ceiling, and graph
artifact retention. Settings marked “restart to apply” affect new sessions;
Graph and Swarm gates are reloaded by the command path.

## AgentGraph

AgentGraph uses a versioned declarative specification. The host parses,
normalizes, validates, hashes, stores, and schedules the graph. GraphSpec v1
uses:

```yaml
apiVersion: bandicot.dev/v1alpha1
kind: AgentGraph
metadata:
  name: inspect-project
  graphVersion: 1
spec:
  objective: Inspect the project and return a structured risk summary.
  execution:
    orchestrationPolicy: standard
    maxDepth: 1
    maxTotalNodes: 1
    maxActiveModelCalls: 1
    disableNestedBandicotAgents: true
    disableProviderMultiAgentForWorkers: true
  budgets:
    maxWallTimeSeconds: 600
    maxInputTokens: 20000
    maxOutputTokens: 5000
    hardStop: false
  defaults:
    reasoningEffort: low
    serviceTier: standard
    capabilityMode: read-only
    timeoutSeconds: 600
    maxToolCalls: 20
    maxInputTokens: 20000
    maxOutputTokens: 5000
  schemas:
    worker-output:
      type: object
      required: [summary]
      properties:
        summary:
          type: string
  nodes:
    - id: inspect
      kind: agent
      objective: Inspect project boundaries, risks, and verification needs.
      definitionOfDone:
        - Return a valid structured NodeOutput.
        - Support the summary with file-based evidence.
      outputSchemaRef: worker-output
      capabilityMode: read-only
      serviceTier: standard
      evidenceRequirements:
        - kind: node-output
          required: true
  edges: []
```

This example deliberately omits a model selector. Immediately before dispatch,
the session backend resolves built-in selectors against the live model catalog.
`worker-light` requires Luna without a silent fallback,
`reducer-balanced` prefers Terra, and `critical-verifier` prefers Sol with the
declared GPT-5.6 fallback. Capability, provider, API backend, hidden-model,
reasoning-effort, image, structured-output, tool, and service-tier constraints
fail closed. Real workers accept read-only or isolated-worktree authority;
unisolated writes and external effects are rejected.
Real graph workers apply the node/default `serviceTier`; the exact-100
benchmark profile pins workers to Standard even when the root session is Fast.

### Commands

```text
/graph status
/graph validate path/to/graph.yaml
/graph preview path/to/graph.yaml
/graph plan path/to/graph.yaml
/graph approval
/graph approve NORMALIZED_GRAPH_HASH
/graph run path/to/graph.yaml
/graph run
/graph pause
/graph resume
/graph drain
/graph cancel
```

`validate` checks the header, references, topology, bounds, authority,
resources, and write conflicts, then returns a normalized hash. `preview` also
shows topology, declared budgets, model selectors, permissions, effects, and
resources.

`plan` validates and stores a run in `AwaitingApproval` state without starting
workers. High-width or side-effecting runs require an approval bound to the
normalized graph, revision, budgets, effects, permissions, repository commit,
and expiry. `/graph approval` prints the non-secret binding and `/graph approve`
persists an explicit hash acknowledgment. The explicit `/graph run` command starts execution
when the session subagent backend is available. If it is unavailable, the run
is stored but not executed, and Bandicot does not substitute fake results.

Real graph workers currently have these enforced limits:

- Read-only capability only.
- No nested Bandicot or provider-hosted orchestration.
- Maximum graph depth of one.
- Bounded active model calls from `maxActiveModelCalls`.
- Resource-claim and overlapping write-set checks in the scheduler.
- Structured NodeOutput validation before output is accepted.
- Stale attempt output is rejected by attempt number.

The scheduler atomically reserves persisted input, output, model-call,
tool-call, attempt, and lease capacity before dispatch. It reconciles actual
usage and conservatively charges the full reservation when usage is missing.
Wall deadlines are absolute persisted timestamps. Retry classifications,
chosen jittered deadlines, Retry-After values, and history are durable. Loop
state, seen keys, deterministic expansion IDs, usage, artifacts, and bounds are
durable. Required compensation runs as a reverse-order saga with its own
leases, retries, budgets, and terminal states.

Host admission uses weighted fair queues, atomic multi-resource claims,
head-of-line bypass, and an interactive reserve. Provider admission starts
conservatively, reserves estimated tokens, consumes typed rate-limit headers,
honors Retry-After, reduces concurrency after 429s, recovers gradually, and
opens a circuit after repeated authentication failures. Swarm starts with a
small canary wave and doubles only after successful batches.

### State and crash recovery

Graph specifications, run status, node attempts, outputs, leases, and ordered
events are stored in a project-local SQLite database. The active run is attached
by both session and repository, so two sessions do not overwrite each other's
active-run selection. Pause, resume, drain, and cancel requests are durable and
an active background scheduler observes them between worker batches.

At process startup the runtime manager scans nonterminal runs, acquires a
coordinator lease, enforces persisted wall deadlines, expires stale worker
leases, reconciles conservative usage, and activates due persisted retries.
The sweep continues in the background, so lease expiry does not require a
manual command. Recovery is idempotent and stale attempts cannot overwrite a
newer attempt.

## Structured ACP and non-interactive control

The same canonical graph service backs slash commands and the structured ACP
methods under `x.ai/graph/`: `draft`, `validate`, `preview`, `run`, `status`,
`explain`, `pause`, `drain`, `resume`, `cancel`, `retry`, `artifact`, `export`,
and `cleanup`. Requests and responses are typed JSON. A required approval is a
structured object bound to the normalized graph hash, revision, budget,
effects, permissions, repository commit, and expiry. Missing or changed
approval fails before dispatch and returns the exact material to sign; the
approval contains no credentials.

The optional Codex app-server adapter remains disabled by default. Its local
fake-server test covers initialize/initialized, account and model preflight,
thread lifecycle, turn lifecycle, structured output, interruption, and
shutdown. A live private Codex executable check is external-only.

## Swarm and the exact-100 benchmark

Swarm is the high-width AgentGraph profile. Offline scale validation remains
available through the fake-backend scheduler benchmark:

```text
/swarm preview --fake
/swarm plan --fake
/swarm run --fake
/swarm benchmark --fake
/swarm benchmark --fake --limit 25
```

The exact-100 benchmark builds 100 independent read-only worker nodes and
checks scheduler width without making provider requests. With the default limit
of 100, the expected peak is 100. A smaller `--limit` demonstrates queueing and
bounded concurrency. The report includes configured limit, peak active workers,
queued workers, terminal counts, duration, and `Backend: fake`.

`/swarm run` without `--fake` uses the same session worker backend as real
AgentGraph execution only when `swarm_enabled` and `live_swarm_enabled` are
both enabled, the active run carries a hard budget, and its bound approval is
valid. The default remains off. The benchmark proves local
graph validation and scheduling behavior only. It does not prove provider
capacity, account quota, model quality, network behavior, live cost, or the
ability to run 100 real agents.

## ACP and headless operation

Fast, Ultra, AgentGraph, and Swarm remain in the shell's ACP command
advertisement. The pager forwards slash commands to the shell rather than the
model. Automation can instead use the structured `x.ai/graph/*` methods listed
above. The structured `run` method accepts the same immutable
`ExecutionApproval` record as the TUI. Without a required approval it returns
the exact approval binding and dispatches no worker. A generic headless or
non-interactive flag is never treated as high-cost approval.

## Optional Codex app-server backend

The optional Codex app-server JSON-RPC worker adapter is implemented and
disabled by default. It checks the configured executable before use and fails
clearly when the executable or protocol handshake is unavailable. Unit tests
use a local fake JSON-RPC process and do not require Codex to be installed.

The adapter supports initialization, thread start/resume/fork, turn start,
turn interruption, notifications, structured final output, cancellation, and
child-process cleanup. It always requests `approvalPolicy = "never"` with the
`read-only` sandbox; enabling it does not broaden worker authority.

Current `/graph run` execution continues to use Bandicot's existing session
subagent backend. The app-server adapter is a separate host-selectable worker
backend and core AgentGraph validation, storage, and scheduling do not depend
on Codex being installed.
