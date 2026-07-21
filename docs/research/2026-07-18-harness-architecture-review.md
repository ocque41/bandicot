# Harness Architecture Review Intake

Date: 2026-07-18

Provenance: user-supplied research and recommendations

Validation status: not independently verified as part of this intake

## Executive Verdict

Grok Build is a strong upstream chassis for Bandicot, but its main comparative
advantage is not general provider support or broad harness maturity. Its most
valuable mechanism is context and execution-state recovery. Its compactor can
combine speculative pre-compaction, multiple fitting and fallback strategies,
cache validation, checkpoints, memory flushing, and reconstruction of active
tasks, subagents, MCP state, TODOs, plans, rules, tools, and edited paths.

The recommended target architecture is:

> A sanitized Grok Build control plane with an event-sourced context kernel,
> deterministic repository intelligence, strict resource budgets, and official
> Codex workers running the available Luna, Terra, and Sol model profiles.

Under a subscription-only configuration, Codex is itself an agent harness. It
should be integrated as a worker runtime rather than disguised as a raw chat
completion provider. Bandicot can control worker lifetime, handoffs, outer
memory, worktrees, budgets, task decomposition, permissions, concurrency, and
model selection, but Codex retains control over its internal prompt and
compaction behavior. Complete control of every model-visible token would require
separately billed API access or local inference.

No public benchmark was identified that directly evaluates the newly published
Grok Build snapshot with GPT-5.6 Sol. Model and architecture conclusions require
Bandicot's own record-and-replay evaluation.

## Assumptions

The recommendations assume:

1. Bandicot is a personal development tool, not a resold service.
2. Routine inference should stay within one ChatGPT Pro subscription.
3. Accepted engineering work per unit of allowance matters more than agent count.
4. Bandicot will maintain a reviewable patch stack against upstream snapshots.
5. Repositories are trusted, while dependencies, build scripts, plugins, MCP
   servers, and generated content are not trusted automatically.

Subscription allowances are finite and may be shared across agentic products.
Any available optional credits would violate a strict subscription-only budget.
Current plan details and model availability must be discovered and revalidated
at implementation time.

## Preserve As Invariants

- Compaction checkpoints, retries, and fallback ladders.
- Reconstruction of tasks, subagents, TODOs, plans, rules, tools, and memory.
- Session event and update persistence.
- The stable `search_tool` and `use_tool` lazy MCP boundary.
- Permission precedence and project trust.
- Kernel-backed sandbox integration.
- Subagent status, resume, cancellation, and background lifecycle.
- Maximum outer subagent depth of one.
- Worktree isolation, after making writer failures fail closed.
- TUI, headless, and ACP boundaries.
- Plan review and inline diff workflows.
- Existing Rust crate boundaries and generated-workspace process.
- Third-party attribution, notices, and original licenses.

"Preserve" means protect the behavior with tests before changing its
implementation. It does not mean freezing individual source lines.

## Comparative Findings

| Harness | Comparative strength | Bandicot lesson |
| --- | --- | --- |
| OpenCode | Provider ecosystem, HTTP/OpenAPI server boundary, SDK ergonomics, declarative agents | Borrow server boundaries and adapters; retain Grok recovery and persistence |
| Codex | Official subscription-backed model access, app-server, SDK, structured output, threads, effort, and sandbox controls | Integrate as a high-level worker backend |
| Claude Code | Mature subagent configuration, scheduling, event integrations, skills, isolation, and turn controls | Borrow role definitions and event-driven scheduling |
| Aider | Graph-ranked repository map, stable prompt layout, architect/editor split | Add a bounded deterministic repository map |
| OpenHands | Append-only event history and pluggable condensers | Derive model context from immutable events |
| SWE-agent | Small model-oriented tools and concise command feedback | Keep file windows, search, edits, and test output bounded |
| Grok Build | Recoverable context transitions and integrated stateful runtime | Keep as the upstream chassis |

The supplied correction notes that current Claude Code documentation permits
nested subagents and documents a much higher default session count. Earlier
research that described Claude as prohibiting nesting is obsolete. Bandicot's
depth-one policy remains the recommended local budget and safety invariant.

## Priority Zero

### Compile Out Unwanted Remote Control

Do not rely on server-side flags to protect local source. Remove or permanently
feature-disable unwanted paths for:

- Trace, session archive, memory archive, and codebase bundle upload.
- Remote xAI feature settings.
- xAI authentication, billing, dashboard, and feedback surfaces.
- Managed xAI MCP gateways.
- Automatic binary replacement.
- Unreviewed remote plugins.
- Unused media generation and deployment integrations.

Retain a local-only observability interface backed by storage under user control.
Add secret canaries and outbound-network integration tests before declaring the
sanitized baseline complete.

### Add A High-Level Worker Backend

Do not fit Codex app-server into the low-level `ChatModel` abstraction. Use a
worker-level interface:

```rust
trait WorkerBackend {
    async fn start(&self, spec: WorkerSpec) -> Result<WorkerHandle>;
    async fn steer(&self, id: WorkerId, input: WorkerInput) -> Result<()>;
    async fn interrupt(&self, id: WorkerId) -> Result<()>;
    async fn status(&self, id: WorkerId) -> Result<WorkerStatus>;
    async fn collect(&self, id: WorkerId) -> Result<WorkerPacket>;
}
```

Candidate implementations:

- `LocalGrokWorkerBackend` for the current agent loop.
- `CodexAppServerBackend` for ChatGPT-managed authentication.
- `DirectApiWorkerBackend` only if separately billed API usage is accepted.
- `LocalModelWorkerBackend` for future local inference.

Prototype with `codex exec` or the official SDK. Move to app-server when
Bandicot requires streamed events, persistent thread control, goals, forks,
approvals, and structured outputs.

### Add Hard Resource Accounting

Add deterministic admission and cancellation above the existing worker
lifecycle:

- Global active-worker limit.
- Per-worker token, turn, tool-call, wall-clock, and output budgets.
- Limits on equivalent errors, unchanged test reruns, and duplicate file reads.
- Cancellation after a defined no-progress window.
- Global subscription allowance accounting based on observed usage.

Default to one active model worker. Permit two only for genuinely independent
work. Permit one writing worker per target branch.

### Make Writer Isolation Fail Closed

Any autonomous or concurrent writer that requests a worktree must stop if
creation or restoration fails. It must never continue in the parent workspace.
Read-only scouts may inspect the parent workspace.

## Priority One

### Projected Compaction

Replace a universal percentage trigger with projected next-turn usage:

```text
projected_usage =
    current_model_visible_tokens
  + p95(next_turn_input_growth)
  + p95(next_tool_output_burst)
  + required_completion_reserve
  + retry_reserve
```

Compact when projected usage exceeds the safe context budget. Initial role
thresholds in the supplied research range from approximately 40 to 60 percent
for soft action and 55 to 68 percent for hard action. These are evaluation
hypotheses, not defaults to adopt without replay evidence.

### Append-Only Event Store

Persist immutable events such as:

```text
UserObjective
ConstraintAdded
PlanAccepted
FileObserved
SymbolLocated
CommandExecuted
TestResult
DecisionMade
PatchCreated
AgentSpawned
AgentCompleted
FailureObserved
CheckpointCreated
```

Every event should carry a stable ID, causal and parent IDs, actor, repository,
worktree, relevant hashes, token estimate, privacy class, active/pinned state,
and source pointers. The transcript becomes a projection. Compaction creates a
new projection and checkpoint without rewriting canonical history.

### Deterministic Repository Intelligence

Build a local index of:

- Files, languages, symbols, signatures, imports, and dependencies.
- Build targets, tests, coverage relationships, and package manifests.
- Entry points and configuration sources.
- Changed files, subsystem ownership, and history hot spots.
- Generated versus maintained source.

A read-only scout should return precise evidence packets. It should not edit.

### Execution-State Memory

Split memory into:

1. Execution state: objective, plan, completed steps, blockers, decisions,
   changed files, and tests.
2. Repository knowledge: architecture, conventions, commands, and recurring
   failures.
3. Historical evidence: old logs, previous implementations, abandoned branches,
   and external research.

Execution state should use a hierarchical active task path rather than semantic
similarity. Semantic retrieval belongs in repository knowledge and historical
evidence. Token-aware, symbol-aware, and AST-aware units should replace generic
character chunks.

### Event-Driven Watchers

Extend host-owned `/loop` with deterministic predicates:

- Process exit.
- File, test, Git HEAD, CI, deployment, log, port-health, or review changes.
- Time deadlines.

Only wake a model after a meaningful state transition or defined exception.
Queueing, retries, budgets, process state, and dependency resolution should stay
deterministic.

## Codex Integration Sequence

1. Start `codex app-server` over stdio or a Unix socket.
2. Use ChatGPT-managed browser or device-code authentication.
3. Initialize the protocol.
4. Discover available models instead of hardcoding identifiers.
5. Start a thread with an explicit directory, sandbox, and approval policy.
6. Set a persisted goal and accounting budget.
7. Start turns with model, effort, summary policy, and output schema.
8. Stream item and tool events into Bandicot's event store.
9. Interrupt on budget or no-progress policy.
10. Save a structured result packet and terminate or archive the thread.

Treat Codex as a worker runtime. A fake chat-completion provider would duplicate
tools, permissions, repository exploration, context management, compaction, and
subagent spawning while making quota attribution unreliable.

## Model Routing Hypothesis

Discover actual model identifiers and capabilities at runtime.

| Role | Initial profile | Effort and escalation |
| --- | --- | --- |
| Request classification | Luna | Low; no escalation |
| Repository scout | Luna | Low; Terra if evidence remains ambiguous |
| Test condensation | Luna | Low |
| Memory extraction | Deterministic, then Luna | Low |
| Planner | Terra | Medium; Sol for architecture-wide work |
| Routine implementation | Terra | Medium; Sol after diagnosed reasoning failure |
| Difficult debugging | Sol | Medium/high; maximum only with a bounded evidence packet |
| Security review | Sol | High, focused pass |
| General review | Terra | Medium; Sol for high-risk code |
| Final integration | Sol | Medium/high |
| Compaction verification | Luna or Terra | Low; Sol only for conflicting state |

Do not use Sol for every scout, summary, test log, or search. Do not enable a
hidden multi-agent or "Ultra" mode by default because it conflicts with outer
admission control and allowance efficiency.

## Context Projection

Never compact away:

- User objective and explicit prohibitions.
- Permission policy and acceptance criteria.
- Current worktree and branch.
- Active plan, blockers, decisions, and test state.
- Worker identities, budgets, and required output contracts.

Assemble worker context in stable order:

1. Pinned control block.
2. Worker contract and budget.
3. Relevant repository-map slice.
4. Execution ledger.
5. Evidence packet with source pointers.
6. Small recent interaction tail.
7. Lazy tool catalog.

Compaction should remove repeated UI chatter, replace large outputs with artifact
pointers, deduplicate reads and commands, collapse completed tool groups into
events, deterministically extract constraints and state, summarize only the
remaining narrative, validate pinned-state coverage, write a checkpoint, and
begin a new phase context. Never recursively summarize a summary without source
events.

For Codex workers, prefer phase turnover: scout, evidence handoff, implementation,
patch/test handoff, and fresh review. Short threads are more observable than one
long thread relying on opaque internal compaction.

## Worker Contract And Circuit Breakers

Workers should return structured status, evidence with file ranges, decisions,
files changed, commands and exit codes, tests, risks, next action, and confidence.
Use turn-level output schemas where supported.

Stop or escalate when:

- Equivalent command output fails three times.
- The same hunk is edited and reverted repeatedly.
- Tests do not change after multiple claimed fixes.
- Repeated reads produce no new evidence.
- Main claims lack source evidence.
- Role budgets are exceeded.
- Required worktree creation fails.
- A worker attempts another delegation layer.
- Projected usage exceeds the safe context budget.
- Repository state does not change during the progress window.

## Implementation Order

1. Tag the exact upstream snapshot.
2. Add local record-and-replay evaluation.
3. Add secret canaries and outbound-network tests.
4. Compile out unwanted remote surfaces.
5. Make sandboxing and network denial the personal default.
6. Make writer worktree failures hard errors.
7. Add `WorkerBackend`.
8. Prototype `codex exec`.
9. Integrate app-server JSON-RPC.
10. Add usage and wall-clock accounting.
11. Add global concurrency and role budgets.
12. Enforce structured worker packets.
13. Build the append-only event store.
14. Project current context from events.
15. Pin constraints and validate coverage.
16. Add projected compaction.
17. Build deterministic repository intelligence.
18. Add cheap scout and condenser roles.
19. Add event-driven watchers.
20. Only then optimize compressor prompts or learned compression.

Do not begin by rewriting the compactor. Establish a sanitized baseline and an
evaluation suite first.

## Evaluation Plan

Use Bandicot's target repositories plus selected public tasks. Cover small edits,
navigation, multi-file features, debugging, refactors, dependency failures,
long-running commands, compaction, interruption/resume, and adversarial
permission and secret cases.

Compare:

- Sanitized upstream policy versus Bandicot policy.
- Fixed threshold versus projected compaction.
- Transcript summary versus event projection.
- Repository map versus no map.
- Single worker versus conditional scout.
- Full tool schemas versus lazy schemas.
- Primary-model versus cheaper-model compaction.
- Shared fallback versus fail-closed worktrees.
- Polling loops versus event-driven watchers.
- Long threads versus phase-oriented handoffs.

Track accepted task rate, human corrections, model usage, usage per accepted
task, compactions, constraint loss, duplicate reads/searches, repeated failures,
budget violations, worker overlap, elapsed time, interventions, interruption
recovery, memory precision/recall, and unapproved egress.

Initial targets to test rather than assume:

- At least 30 percent lower usage per accepted task.
- No material acceptance decline.
- Zero pinned-constraint loss.
- Zero silent writer worktree fallback.
- Zero unapproved outbound destinations.
- More than 95 percent worker budget adherence.
- Equivalent-failure detection within three attempts.
- Evidence and test status in every result packet.

## Research Claims Requiring Revalidation

Before implementation, verify against checked-out source and first-party docs:

- Current Grok Build upload, remote-settings, sandbox, memory, and compaction paths.
- Current ChatGPT plan allowances and whether products share an agentic pool.
- Codex app-server protocol, authentication, goals, model discovery, output schema,
  token reporting, thread, fork, and sandbox behavior.
- Availability and identifiers of Luna, Terra, Sol, and any multi-agent mode.
- Current Claude subagent and scheduled-task limits.
- Reported benchmark values from ACON, MAGE, FastContext, Repository Intelligence
  Graph, Governance Decay, Cost of Consensus, and compression research.
- OpenCode issue reports as individual failure cases, not prevalence evidence.

## Supplied Sources

- https://developers.openai.com/codex/app-server
- https://developers.openai.com/codex/non-interactive-mode
- https://github.com/xai-org/grok-build
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/13-memory.md
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/17-sessions.md
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/20-background-tasks.md
- https://code.claude.com/docs/en/sub-agents
- https://code.claude.com/docs/en/scheduled-tasks
- https://opencode.ai/docs/server/
- https://aider.chat/docs/repomap.html
- https://openai.com/index/gpt-5-6/
- https://help.openai.com/en/articles/20001354-gpt-56-in-chatgpt
- https://www.theverge.com/ai-artificial-intelligence/965600/spacexai-grok-build-repository-upload
- https://github.com/anomalyco/opencode/issues/17557
- https://arxiv.org/abs/2307.03172
- https://arxiv.org/abs/2510.00615
- https://arxiv.org/abs/2604.02985
- https://arxiv.org/abs/2605.00914
- https://arxiv.org/abs/2606.06090
- https://arxiv.org/abs/2606.14066
- https://arxiv.org/abs/2606.22528

The supplied Help Center URL for general plan information contained a malformed
encoded suffix and is intentionally not reproduced. Locate the current official
plan page before relying on its claim.
