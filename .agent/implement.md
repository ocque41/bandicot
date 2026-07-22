# Implementation Notes

## Success Criteria Map

- Fast provider wire: missing at start; owned by `provider_fast`.
- Service-tier/provider capability types: missing at start; owned by `provider_fast`.
- Hosted provider multi-agent capability: missing at start; owned by `provider_fast`.
- AgentGraph core: missing at start; owned by `agentgraph_core`.
- Durable graph store and scheduler: missing at start; owned by parent/root implementation.
- Resource controls, worker backend, exact-100 fake benchmark: missing at start; owned by parent/root implementation.
- Slash commands, TUI/ACP/headless status, docs: missing at start; owned by parent/root implementation.
- Fast requested preference resume/fork persistence: missing after initial Fast command slice; implemented in summary/session persistence with effective support recomputed on restore and model/provider changes.

## Source Stability

- Current source is stable for root-captured validation.
- Assumption: stable means reconciled implementation/docs/reports, not full validation pass.

## Assumptions

- The local checkout is authoritative.
- Existing modified/untracked files are intentional input from root/user.
- No live provider capacity can be assumed; all 100-worker proof must use fake/local backends.
