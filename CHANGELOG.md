# Bandicot Changelog

This changelog records changes maintained by the Bandicot fork. Upstream Grok
Build release notes remain under `crates/codegen/xai-grok-shell/`.

## Unreleased

### Changed

- Made `install-bandicot.sh` canonical and retained `install-openai.sh` only as
  a compatibility wrapper.
- Consolidated the installed command, payload, configuration, and sessions
  under `bandicot`, `~/.local/libexec/bandicot`, and `~/.bandicot` while keeping
  compatible internal identifiers unchanged.
- Added verified, source-preserving migration from prior Bandicot homes and a
  dry-run, provenance-aware uninstaller for old fork-owned aliases and payloads.
- Changed shell completion generation to use the public `bandicot` command.
- Replaced the upstream welcome logo with width-safe, reference-derived large
  and compact Braille Bandicot artwork, a restrained ear-twitch animation, and
  an ASCII narrow/legacy-console fallback.
- Updated terminal titles, notifications, crash headings, and welcome copy to
  use the Bandicot product name while retaining compatible internal IDs.
- Explicit compact `/loop` intervals now bypass model scheduling and create
  durable recurring tasks through a shared host API used by pager and shell;
  natural-language and dynamic forms remain model-assisted.
- Scheduled prompts now load global and project `.bandicot/loop.md` guidance.

### Documentation

- Added the Bandicot documentation index, implementation ledger, research
  summary, and initial architecture decisions.
- Recorded the supplied comparative harness research and added a separate
  proposed roadmap for privacy hardening, worker backends, budgets, event-sourced
  context, projected compaction, repository intelligence, and event watchers.

### Added

- Completed the host-owned Fast, Ultra, AgentGraph, and Swarm control plane:
  durable budgets, leases, startup recovery, persisted retries, bounded loops,
  saga compensation, fair host admission, adaptive provider capacity, live
  catalog model routing, typed settings, structured ACP controls, bound
  non-interactive approval, and offline exact-100 verification.
- Added typed `/plan` + `/goal` + `/loop` composition with canonical
  `/loop 10m --plan --goal objective` and compatibility
  `/plan /goal /loop 10m objective` forms. Composed workflows reuse one approved
  plan, activate goals through the host, keep at most one durable one-shot
  wakeup pending, reject overlapping runs, cancel on verified completion, and
  restore fail-closed from persisted session state.
- Added `bandicot migrate-resources` with dry-run support, project/global scope
  preservation, enabled-plugin resources, staged copies, deterministic
  source-qualified collision handling, Claude-agent manifest adaptation,
  idempotent managed updates, and secret-free provenance manifests.
- Added canonical `.bandicot` and `~/.bandicot` skill/agent discovery ahead of
  legacy `.grok` paths while retaining compatibility.
- Added provider capabilities and configurable HTTP wire quirks for auth,
  Chat Completions token limits, reasoning fields, stream options, and tool
  choice.
- Added a secret-free OpenCode Go profile covering every subscription model
  through one Chat Completions endpoint, plus an Ollama profile, mock
  streaming, and tool-continuation acceptance coverage. The OpenCode Go
  profile replaces OpenCode Zen, whose profile and setup scripts were removed.
- Deprecated the standalone Cerebras profile, retaining it as a secret-free
  reference stub without active model entries.
- Merged all supported provider entries into the default Bandicot model catalog
  so `/models` exposes OpenAI, OpenCode Go, Ollama, and Apple without
  replacing the active profile.
- Capped OpenAI profiles at a 362K model-visible context
  and configured compaction at approximately 184K, leaving other providers on
  model-appropriate thresholds.
- Added a secret-free Apple Foundation Models profile and an isolated native
  Swift helper installed under Bandicot's libexec directory. The non-HTTP
  transport includes runtime availability gates, framed stdio, cancellation
  cleanup, normalized streaming, bounded errors, transcript mapping, dynamic
  JSON schema output, and conservative tool/image suppression.

### Security

- Isolated generic loopback endpoints from trusted cli-chat-proxy headers and
  prevented custom catalog route overrides from implicitly retaining inherited
  credential sources.
- Changed new Keychain writes to the `bandicot.openai` service while retaining a
  read-only fallback for the legacy fork service during migration.
- Added an explicit legacy-only cleanup mode that can provenance-check fork
  aliases and remove npm-owned `@xai-official/grok` without deleting Bandicot or
  any user data.

### Fixed

- Claude skill imports now populate `[skills].paths`, the field consumed by
  production discovery, rather than `[paths].extra_skill_dirs`.
- Scheduler persistence now records durable create, delete, fire, expiry, and
  missed-task mutations correctly across restart without catch-up fanout.
