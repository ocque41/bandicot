# Goal

Implement the Bandicot control-plane goal from the attached contract end to end where locally possible:
Fast Mode, Ultra Mode, AgentGraph, Swarm, exact-100 fake benchmark, durable execution, resource control, recovery, UI/ACP/headless surfaces, documentation, and verification.

## Source Of Truth

- `/Users/miguel/.codex/attachments/27a8a857-af32-4137-bc21-345c476d8df5/pasted-text.txt`
- `out/goal-triad/gap_scan.md`
- `out/goal-triad/architecture_solidification_ideas.md`
- `docs/changes/2026-07-21-control-plane-implementation.md`
- `.codex/pipelines/goal-triad/stages/03-sure-polish.toml`
- Current ADRs under `docs/decisions/`

## Non-Goals

- Do not run a live 100-agent/provider test.
- Do not edit generated root `Cargo.toml`.
- Do not enable recursive model-agent fan-out.
- Do not enable Fast automatically through Ultra or Swarm.
- Do not broaden worker permissions or always-approve behavior.

