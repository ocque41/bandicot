# Grok Build Fork Audit Summary

Date: 2026-07-18

## Scope

This record preserves the actionable conclusions from the research supplied for
the Bandicot fork. Claims that affect implementation must be rechecked against
the checked-out source and first-party provider documentation.

The expanded architecture and harness comparison supplied later the same day is
preserved in
[Harness Architecture Review Intake](2026-07-18-harness-architecture-review.md).
That intake distinguishes supplied claims from independently verified facts and
feeds the proposed control-plane roadmap.

## Preserve

- Session and conversation invariants
- Subagent lifecycle, cancellation, resumption, and worktree support
- ACP, headless, permission, checkpoint, and recovery boundaries
- Local code graph and TUI review surfaces
- Compaction retry and failure-recovery mechanics

## Change

- Replace model-mediated explicit scheduling with host scheduling.
- Replace bytes-per-four accounting with provider usage plus calibration.
- Move authoritative task state outside recursive prose summaries.
- Reduce default verifier fanout and use deterministic evidence first.
- Store large tool outputs as artifacts and inject compact digests.
- Route models and effort by role rather than using the strongest model for all
  work.
- Remove or compile-disable unwanted remote export and telemetry surfaces.
- Keep Bandicot's outer subagent depth at one. Current Claude Code documentation
  permits nested subagents, correcting the earlier comparison assumption.

## Scheduling Research

Claude Code documents `/loop` as session-scoped scheduling with a one-minute
minimum, 50-task limit, seven-day recurring expiry, no catch-up fanout, and
execution between turns. It supports fixed intervals, dynamically selected
delays, a maintenance prompt, project/user `loop.md`, and cancellation.

Bandicot will copy the user experience and constraints while keeping explicit
interval parsing and scheduler creation deterministic in the host.

## Provider Research

- Cerebras exposes OpenAI-compatible Chat Completions with streaming, function
  tools, `max_completion_tokens`, and model-dependent reasoning effort.
- OpenCode Zen exposes Responses, Messages, Chat Completions, and a model
  catalog under `https://opencode.ai/zen/v1`.
- Ollama exposes compatible Chat Completions and stateless Responses locally,
  with model-dependent tool and reasoning support.
- Apple Foundation Models requires a native macOS integration rather than the
  current HTTP/SSE-only sampler path.

## Security

Never place API keys in repository files. Keys pasted into chat or logs must be
treated as compromised and rotated. Provider credentials must not cross endpoint
boundaries, and migration tools must not inspect credential stores when scanning
skills or agents.

## Source References

- https://code.claude.com/docs/en/scheduled-tasks
- https://opencode.ai/docs/zen/
- https://inference-docs.cerebras.ai/api-reference/chat-completions
- https://docs.ollama.com/api/openai-compatibility
- https://developer.apple.com/documentation/foundationmodels
