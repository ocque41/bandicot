# 0003: Provider Transport Boundary

Status: accepted

## Context

The sampler supports three HTTP protocols through an exhaustive enum. Provider
identity is mostly represented by an xAI boolean, which cannot express model
capabilities or protocol quirks and cannot support native Apple inference.

## Decision

Separate provider capabilities, wire quirks, credential references, and
inference transport. Preserve existing HTTP protocol implementations behind an
HTTP transport and implement the macOS-native Apple transport as an isolated
Swift stdio helper.

HTTP model configuration carries an explicit auth scheme, tool/image
capabilities, Chat Completions token-field spelling, reasoning-field spelling,
and optional-field emission. These facts are configuration, not hostname
guesses. Loopback alone never grants cli-chat-proxy trust.

## Consequences

- Cerebras, OpenCode Zen, and Ollama can reuse existing HTTP protocols safely.
- Unsupported tools and modalities are not advertised.
- Credentials remain owned by their provider boundary.
- Apple Foundation Models does not require pretending to be an HTTP API.
- Native helper crashes and Swift runtime linkage remain outside the Rust
  process; one process per request gives cancellation a reliable cleanup seam.
- Custom catalog routes own their credential references; replacing a route does
  not retain credentials from the inherited route implicitly.
