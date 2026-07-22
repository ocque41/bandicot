# Universal Context Compaction

Status: verified

Owner: Bandicot shell and common compaction maintainers

## Problem and intended outcome

Bandicot now uses one host-controlled compaction policy for every model. Model
profiles still describe provider capabilities, but they no longer decide when a
session is safe to compact.

## Scope and non-goals

This change covers host-textual full-replace compaction, safe turn boundaries,
runtime prompt management, durable replacement, output reserves, large tool
results, and privacy defaults. Provider-native and legacy two-pass compaction
remain available only as explicit operator experiments; this change does not
claim equivalent safety for those experimental paths.

## Runtime policy

| Session | Logical cap | Soft trigger | Summary cap | Post-compact target |
| --- | ---: | ---: | ---: | ---: |
| Main | 258,000 | 129,000 | 8,192 | 48,000 |
| Subagent | 128,000 | 64,000 | 4,096 | 24,000 |

The effective window is the smallest verified limit from the logical policy,
model/provider metadata, and the live gateway response. A lower live limit is
accepted immediately. An unverified increase is ignored.

Crossing the soft trigger marks compaction as pending and allows one complete
model sample. Tool calls and results are kept as one indivisible batch. Chat-only
turns compact before the request after that sample. A hard guard reserves 8,192
tokens for output and 8,192 tokens for safety.

The state transition is `Healthy -> Pending -> Compacting -> Healthy`. A failed
automatic attempt returns to `Pending` for a bounded retry or enters
`Suppressed` for a deterministic failure. The hard guard can override
suppression.

## Summary safety

- The summary model receives no tools.
- Provider-native compaction is off unless
  `BANDICOT_PROVIDER_NATIVE_COMPACTION=1` is explicitly set.
- Legacy two-pass compaction is off unless
  `BANDICOT_TWO_PASS_COMPACTION=1` is explicitly set.
- Truncated, tool-using, incomplete, malformed, or structurally incomplete
  summaries are rejected before history replacement.
- The replacement must fit the session low-water target, including the normal
  turn's tool-definition cost.
- A durable checkpoint is written and acknowledged before live history is
  replaced.

Raw compaction request artifacts are disabled by default because they contain
the transcript. Operators can opt in with
`BANDICOT_CAPTURE_RAW_COMPACTION=1` when a secure debugging workflow requires
them.

Tool results larger than 64 KiB for main sessions or 32 KiB for subagents are
stored under `.bandicot/artifacts/tool-results/`. The conversation retains a
bounded envelope with the byte count, SHA-256 digest, short summary, and limited
head and tail excerpts. Files are created with owner-only permissions on Unix.

## Runtime prompt

Prompt selection order is:

1. An explicit path supplied to the runtime prompt store.
2. `BANDICOT_COMPACTION_PROMPT`.
3. `~/.bandicot/prompts/compaction.md`.
4. The packaged prompt.

Runtime files must be UTF-8, at most 64 KiB, contain exactly the four approved
variables, and contain all nine required summary headings. The immutable safety
guard is always added by code. Invalid startup files fall back to the packaged
prompt. Invalid reloads retain the last-known-good prompt.

Use `/reload-prompts` to force validation and reload. The command reports the
SHA-256 digest of the active template.

## Operator checks

- `cargo test --locked -p xai-grok-compaction --lib`
- `cargo check --locked -p xai-grok-shell --lib`
- `git diff --check`

## Security and privacy impact

The summary sampler cannot call tools. Runtime prompts cannot replace the
code-owned injection guard. Checkpoints and externalized tool artifacts use
owner-only permissions on Unix. Raw transcript artifacts require explicit
operator opt-in.

## Acceptance criteria and observed results

- Exact main and subagent policy values: implemented with focused unit tests.
- One-sample pending behavior and complete tool-batch boundary: implemented with
  pure transition tests and turn-loop integration.
- Fail-closed summary validation: implemented with focused malformed-output
  tests.
- Runtime prompt validation and last-known-good behavior: all common compaction
  tests pass.
- Durable checkpoint before replacement: implemented through a persistence ACK.
- Full Bandicot shell library check: passed.

## Implemented changes and deviations

The runtime store supports an explicit prompt path, but the current shell entry
point exposes file selection through `BANDICOT_COMPACTION_PROMPT` and the user
prompt path. A dedicated process-level `--compaction-prompt` flag is not added
because the ACP shell does not own the top-level command-line parser.

## Review checkpoints

Re-run the operator checks after changes to model limits, sampler completion
metadata, persistence ordering, tool batching, or runtime prompt structure.
