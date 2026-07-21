# 0004: Apple Foundation Models Stdio Bridge

Status: accepted

## Context

FoundationModels is a Swift framework available on macOS 26 and has runtime
availability states beyond OS version. Linking Swift directly into the Rust
payload would couple release builds, process lifetime, and crash behavior to the
Swift runtime. Bandicot also needs streaming cancellation and cross-platform
protocol tests even though real inference is macOS-only.

## Decision

Run a small native Swift executable as one child process per inference request.
Exchange versioned JSON messages in 4-byte big-endian length-prefixed frames over
stdin/stdout. Keep frame and error sizes bounded, close stdin after the request,
discard stderr, and configure kill-on-drop so cancelling the Rust stream cleans
up the helper.

Represent transport separately from the HTTP `ApiBackend`. The Apple transport
never constructs an HTTP request, resolves credentials, or forwards headers.
Map conversation history into FoundationModels `Transcript` entries and
normalize cumulative snapshots into sampler text deltas.

Use public dynamic `GenerationSchema` support for the documented JSON Schema
subset. Suppress tools and images: FoundationModels tools execute inside the
Swift session and cannot satisfy Bandicot's external tool-call pause/resume
contract without changing tool semantics.

## Consequences

- Swift build/runtime failures are isolated from Bandicot's Rust process.
- macOS/model eligibility is checked at runtime with actionable bounded errors.
- Non-macOS builds retain the protocol implementation and tests but return a
  clean unsupported-platform error before spawning anything.
- Cerebras, OpenCode, Ollama, and all other HTTP routes keep their existing
  clients, protocols, credentials, and tests.
