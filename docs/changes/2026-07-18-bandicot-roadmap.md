# Bandicot Foundation Roadmap

Status: verified

Approved: 2026-07-18

## Outcome

Turn this fork into a Bandicot-only local coding harness with host-owned
scheduling, composable long-running workflows, portable skills and agents, and
provider-neutral inference.

## Constraints

- Preserve Bandicot sessions, configuration, source, and imported resources.
- Do not delete Claude or Codex source resources during migration.
- Do not store API keys in source, generated profiles, logs, or tests.
- Keep upstream license and third-party notices.
- Keep internal upstream crate and wire identifiers when renaming them would
  break compatibility or create unnecessary merge churn.
- Require plan approval before a composed plan/goal/loop starts implementation.

## Delivery Ledger

| Phase | Status | Change |
| --- | --- | --- |
| Documentation | implemented | Added the fork ledger, ADRs, research summary, and changelog |
| Identity | implemented | Made `bandicot` and `~/.bandicot` canonical for installation and user state |
| Installation | implemented | Added verified legacy migration and provenance-aware alias cleanup |
| Skills | implemented | Copy owned and enabled Claude/Codex resources while preserving scope |
| Loop | implemented | Explicit compact schedules create durable jobs directly in the host |
| Workflow | implemented | Unified `/plan`, `/goal`, and `/loop` lifecycle around one approved plan |
| Providers | implemented | Added capability metadata, wire quirks, and auth isolation |
| Cerebras | implemented | Added Chat Completions profile and mock wire coverage |
| OpenCode Zen | implemented | Added per-model protocol catalog and bearer authentication |
| Ollama | implemented | Added no-auth loopback profile and tool continuation coverage |
| Apple | implemented | Added a macOS-native Foundation Models transport through an isolated Swift stdio helper |
| Visual identity | implemented | Replaced public Grok text and logo assets with reference-derived Bandicot artwork |

## Host Workflow Pseudocode

```text
parse slash input into WorkflowSpec
validate schedule, budget, and capability constraints
if plan requested:
    create canonical workflow plan
    enter AwaitingPlanApproval
    stop
on approval:
    leave restrictive plan mode
    activate goal execution
while workflow is active:
    execute one bounded goal round
    collect deterministic evidence
    verify completion
    if complete:
        cancel pending wakeup
        mark Complete
    else if a delay is useful:
        schedule one wakeup
        mark WaitingForWakeup
    else:
        continue within the current bounded run
```

## Skill Migration Pseudocode

```text
scan user roots and enabled plugin resources
classify each source as project or global
deduplicate canonical paths and versioned plugin snapshots
parse and validate manifests without reading credential stores
plan collision handling
copy complete resources into adjacent staging directories
adapt names, model identifiers, tools, and path variables
validate using production discovery
atomically promote staged resources
write a secret-free provenance manifest
leave every source unchanged
```

## Provider Pseudocode

```text
resolve model and provider configuration
resolve credential reference without copying the credential
derive ProviderCapabilities and WireQuirks
advertise only supported tools and modalities
convert ConversationRequest through the selected transport
normalize provider events into SamplingEvent
record provider-reported usage
never forward provider-specific credentials or headers across boundaries
```

## Acceptance Criteria

- `bandicot` is the only product command installed on `PATH`.
- Existing Bandicot configuration and sessions remain usable after migration.
- Imported skills and agents are discoverable from their preserved scope.
- Explicit `/loop 5m task` creates no model turn before scheduling.
- Scheduler deletion, expiry, and fire state survive restart correctly.
- Combined plan/goal/loop waits for approval and stops after verified success.
- Provider tests cover authentication isolation and a complete tool round trip.
- No credential appears in repository changes or test fixtures.

## Verification Evidence

Identity and installation phase evidence:

- `install-bandicot.sh` is canonical; `install-openai.sh` only delegates legacy automation.
- The installed executable layout contains only `~/.local/bin/bandicot` and
  `~/.local/libexec/bandicot/bandicot`.
- The launcher generates `bandicot` completions and internally exports
  `GROK_HOME=~/.bandicot` without renaming upstream crates or wire IDs.
- Migration copies and verifies prior Bandicot homes before activation and
  leaves all source data intact.
- `uninstall-bandicot.sh` defaults to dry-run, checks provenance before removal,
  preserves every data home, and removes official `@xai-official/grok` only with
  the explicit `--remove-official-grok` option.

Skill and agent migration evidence:

- `bandicot migrate-resources --dry-run` preflights project/global Claude,
  Codex, agent, legacy Grok, and enabled-plugin resources without writing.
- Migration copies complete skill directories and agent Markdown through
  adjacent staging, validates portable names, and leaves every source intact.
- Existing unmanaged destinations and locally modified managed destinations
  abort before writes. Duplicate content is collapsed, user-owned resources keep
  the canonical name, and distinct enabled-plugin skills receive deterministic
  source-qualified names. Repeated runs are idempotent, while changed sources
  update only manifest-owned destinations.
- Project and global `.bandicot/migration-provenance.json` files contain only
  resource identity/scope, source and target paths, content hashes, and optional
  plugin name/version. Credential stores and provider configuration are outside
  the scanner.
- Enabled resources come from the resolved plugin registry, which selects one
  active install and excludes stale duplicate cache versions.
- Claude skill-directory import now writes `[skills].paths`, matching production
  discovery instead of the unused `[paths].extra_skill_dirs` field.
- Foreign Claude agent manifests are normalized into Bandicot's portable schema
  without changing their source prompt, and unsupported foreign model pins and
  MCP wildcards are not promoted into the destination.
- Unit and integration fixtures use temporary home/project/plugin trees only.
- The real migration copied 230 resources. A subsequent dry run reported
  `0 copied, 0 updated, 230 unchanged, 0 invalid`.

Initial host-owned loop evidence:

- `/loop <number><s|m|h|d> <prompt>` is parsed by a shared host parser and
  reaches the scheduler through `x.ai/scheduler/create` without a model turn.
- Pager, shell, and the model tool use one scheduler-handle create function.
  Natural-language and dynamic forms retain the model-assisted path.
- Explicit host loops are durable and fire immediately while retaining the
  one-minute minimum, 50-task limit, seven-day expiry, and no catch-up fanout.
- Scheduler state persists durable create, delete, fire, missed one-shot cleanup,
  and expiry mutations. Shutdown no longer drains authoritative scheduler state.
- Global `~/.bandicot/loop.md` and project-ancestor `.bandicot/loop.md` guidance
  is appended to scheduled prompts in global-to-project order.
- Scheduler/parser checks and 48 targeted tests pass, including restart tests
  for durable create/delete, last-fire state, expiry, and overdue recurrence.

Approved workflow evidence:

- Canonical `/loop 10m --plan --goal objective` and compatibility
  `/plan /goal /loop 10m objective` resolve to the same typed workflow action.
- The workflow enters host plan mode and cannot fall open when no approval
  client is available. Goal setup occurs only after a successful approved plan
  exit, after restrictive plan mode is left.
- The approved session `plan.md` is attached directly to goal state, so the goal
  planner does not create a second plan artifact.
- Each incomplete completed run creates one durable one-shot scheduler task.
  A matching scheduler wake resumes the goal through the host API, stale or
  overlapping wakes are skipped, and verified completion deletes any pending
  task before marking the workflow complete.
- Workflow phase, interval, objective, pending task ID, and overlap gate persist
  with plan-mode state. Restore clears transient in-flight ownership and safely
  reconciles a missing wake or an already-complete goal.

Visual identity now uses reference-derived Braille bandicoot artwork in large
and compact welcome layouts, with an ASCII-safe narrow and legacy-console
fallback. A restrained two-beat ear twitch runs with the existing slow shimmer.
The user accepted the installed result. Terminal titles,
desktop notification branding, crash headings, and welcome copy now identify the
product as Bandicot. Internal crates, protocol strings, serialized IDs,
attribution, installer paths, providers, and scheduler behavior remain unchanged.
- Crash report formatting tests pass (`2 passed`). The final logo suite passes
  (`13 passed`), including responsive selection, legacy fallback, shimmer, and
  ear-twitch behavior.

Provider foundation evidence covers raw profile parsing, provider-owned
credentials, no-auth loopback isolation, text streaming, and a Chat Completions
tool-call/result continuation against local mocks. Apple adds a separate native
transport, bounded framed protocol mocks, child cleanup on stream cancellation,
availability/version gates, cumulative snapshot normalization, dynamic schema
support, and explicit tool/image capability suppression. The Swift package
compiles and its protocol tests pass with Xcode 26.4 on macOS 26.5; the system
model reports `available` on the verification machine.

The active default profile now contains 13 selectable entries spanning OpenAI,
Cerebras, OpenCode Zen, Ollama, and Apple. Profile parsing, provider contracts,
the installed-binary catalog test, and `/models` allowlist coverage pass. The
installed `~/.bandicot/config.toml` matches the checked-in canonical profile.

Follow-on harness research is recorded separately rather than represented as
completed foundation work. See the
[research intake](../research/2026-07-18-harness-architecture-review.md) and
[proposed control-plane roadmap](2026-07-18-control-plane-next.md).

Final machine verification:

- `command -v bandicot` resolves to `~/.local/bin/bandicot`.
- Neither `grok-openai` nor `grok` resolves on `PATH`.
- Global npm no longer contains `@xai-official/grok`.
- Active configuration, sessions, imported resources, and provenance are under
  `~/.bandicot`; legacy source data was preserved.
- Scheduler tests passed (`42`), workflow parser/state tests passed (`8` shared
  parser and `4` shell), provider profile tests passed (`4`), provider wire tests
  passed (`4`), and Swift protocol tests passed (`2`).
- `cargo check --workspace`, `cargo fmt --all -- --check`, `git diff --check`,
  shell workflow tests, and the script installer/update suite passed.

## Deviations

The upstream snapshot marker remains `.grok-openai-upstream`; changing this
internal update artifact is outside the public-identity phase and would add
unnecessary migration risk.

Portable resource migration rejects symlinks and special files rather than
following them into unowned or credential-bearing locations.

The composed workflow state is persisted with the existing plan-mode snapshot
rather than through a parallel workflow database. Scheduler tasks and goal state
continue to use their existing persistence owners.

The optimized `release-dist` build reached final application compilation but
did not finish linking within the execution environment's repeated 20-minute
command limit. The globally installed binary is therefore the current verified
development-profile build; this affects binary size and startup performance,
not feature behavior. Producing and replacing it with a release artifact remains
a packaging follow-up.
