# Bandicot Documentation

Bandicot documentation separates current behavior, planned work, durable
architecture decisions, and chronological user-visible changes.

## Index

- [OpenAI and ChatGPT integration](OPENAI.md)
- [Provider profiles and Apple on-device inference](PROVIDERS.md)
- [Fast, Ultra, AgentGraph, and Swarm](CONTROL_PLANE.md)
- [Updating from upstream](UPDATING.md)
- [Grok Build fork audit](research/grok-build-audit.md)
- [Harness architecture research intake](research/2026-07-18-harness-architecture-review.md)
- [Change records](changes/README.md)
- [Current implementation ledger](changes/2026-07-18-bandicot-roadmap.md)
- [Proposed control-plane roadmap](changes/2026-07-18-control-plane-next.md)
- [Control-plane implementation ledger](changes/2026-07-21-control-plane-implementation.md)
- [Architecture decisions](decisions/README.md)
- [Fork changelog](../CHANGELOG.md)

## Recording Changes

Every substantial fork change must have a change record before implementation.
The record must contain scope, non-goals, affected paths, pseudocode where it
clarifies behavior, security impact, acceptance criteria, and verification
evidence. Update the same record when implementation differs from the plan.

Record reviewable engineering rationale and evidence. Do not record secrets,
credentials, private reasoning traces, or copied model chain-of-thought.
