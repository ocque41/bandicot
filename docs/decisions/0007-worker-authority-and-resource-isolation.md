# 0007: Worker Authority and Graph Resources Are Explicitly Isolated

Status: accepted

## Context

Read-only subagent filtering currently retains task lifecycle and orchestration tools. Git worktrees isolate files but do not isolate ports, databases, builds, test processes, browsers, MCP servers, deployments, or external APIs.

## Decision

Every graph node declares a capability ceiling, explicit tool allowlist and denylist, environment allowlist, normalized read and write sets, network policy, credential references, external effects, and resource claims. The host enforces `child authority <= parent authority` and removes nested Bandicot and provider-hosted orchestration tools from graph workers.

Any number of read-only workers may run within resource limits. At most one unisolated write-capable worker may run for an affected repository workspace. Additional concurrent writers require worktree isolation and disjoint declared write sets, or an explicit serialized integration policy. Shared manifests, lockfiles, migration roots, generated-schema roots, and release configuration are exclusive resources.

Countable resources use bounded permits. Exclusive resources use keyed locks. Initial resource classes include model calls, shell commands, Cargo builds, test suites, web searches, browsers, unisolated writers, worktree writers, ports, databases, MCP servers, deployment targets, and external APIs. Resource acquisition and release are durable graph transitions.

Workers receive an allowlisted environment rather than the complete parent environment. Credentials are referenced, not embedded. Deployment, publication, destructive actions, database migrations, credential changes, and other high-risk effects require host-verified approval facts.

## Consequences

- Read-only mode alone is not an orchestration safety boundary.
- A 100-worker model phase cannot create 100 concurrent builds or test suites.
- Plan Mode rejects write-capable graph nodes.
- Stale writers require isolated-environment inspection or discard before retry.
- Worktree cleanup is explicit for success, failure, cancellation, stale attempts, crashes, and retained diagnostics.
