# 0006: Orchestration Policies and Service Tiers Are Orthogonal

Status: accepted

## Context

Bandicot exposes several independent choices that could otherwise be confused by names such as Fast, Ultra, or Swarm.

## Decision

Bandicot represents these axes independently:

- Orchestration policy: Standard, Ultra, or Swarm.
- Graph representation: AgentGraph for Ultra proposals and explicit Swarm runs.
- Service-tier preference: Inherit, Standard, or Fast.
- Reasoning effort: provider-advertised values such as none, minimal, low, medium, high, xhigh, and max.
- Collaboration mode, permission mode, sandbox, tools, model, provider, and approval policy remain separate state.

Fast means a service-tier preference. For a supported OpenAI Responses request its wire value is `service_tier: "priority"`. Explicit Standard is distinct from Inherit and prevents a catalog default from silently enabling Fast.

Ultra means root-only proactive orchestration using a small validated AgentGraph when parallel work is useful. It is not Plan Mode and does not set a reasoning value. Swarm means explicit user-authorized execution of a validated graph. Neither policy enables Fast, YOLO, always-approve, broader tools, or a weaker sandbox.

Requested intent is persisted separately from effective resolution. Model or provider changes recompute effective support without discarding requested intent.

## Consequences

- Standard/Standard, Standard/Fast, Ultra/Standard, and Ultra/Fast combinations are supported.
- Every Swarm node resolves its own model, effort, capabilities, and service tier.
- Unsupported Fast resolves visibly to Standard while preserving the request.
- UI status reports requested and effective values separately.
