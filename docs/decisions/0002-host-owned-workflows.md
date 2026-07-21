# 0002: Host-Owned Workflows

Status: accepted

## Context

Historically, explicit `/loop` schedules were interpreted by a model, scheduled
execution depended on a pager driver, and `/plan` and `/goal` owned separate plans.

## Decision

The host parses explicit schedules, owns scheduler persistence and wakeups, and
represents plan, goal, and loop behavior in one typed workflow lifecycle. A
composed workflow pauses for plan approval before implementation.

## Consequences

- Model tools and slash commands call one scheduler service.
- Scheduled work can resume without a connected TUI driver.
- One workflow plan becomes the source of truth.
- Goal completion cancels pending wakeups.
- Durable one-shot wakeups are scheduled only after an incomplete run, so a
  workflow never owns more than one pending wakeup.
