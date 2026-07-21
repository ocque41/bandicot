# Provider Profiles

Bandicot includes secret-free profiles for direct OpenAI, OpenCode Go,
Ollama, Apple on-device Foundation Models, and the local CLIProxyAPI
Codex-plan route. A deprecated Cerebras profile is retained for reference
only. Profiles contain only credential references; API keys remain
in their provider-specific environment variables. The Apple profile has no
credential reference because it performs no network inference.

## Profiles

| Provider | Profile | Credential | Protocols |
| --- | --- | --- | --- |
| OpenCode Go | `config/opencode-go.toml` | `OPENCODE_GO_API_KEY` | Chat Completions |
| Ollama | `config/ollama.toml` | none | Chat Completions, stateless Responses |
| Apple Foundation Models | `config/apple-foundation-models.toml` | none | Native Swift stdio bridge |
| Cerebras (deprecated) | `config/cerebras.toml` | `CEREBRAS_API_KEY` | none (reference stub) |

Use a profile as the effective `config.toml`, or copy only the model entries
you need into your existing configuration. Do not add key values to these
files. Models and limits change over time; verify the selected model is enabled
for your account or pulled into Ollama.

## Multi-Provider Setup

The default Bandicot profile (`config/openai.toml`) includes model entries for
OpenAI, OpenCode Go, Ollama, and Apple. The launcher loads every
provider key it can find, so one Bandicot installation can expose all accounts
at once through the model picker.

Store whichever keys you have in macOS Keychain, then reinstall:

```sh
./scripts/setup-openai-key.sh        # OpenAI Platform
./scripts/setup-opencode-go-key.sh   # OpenCode Go
./scripts/install-bandicot.sh
bandicot models
```

On other platforms, set the provider keys only in the process that launches
Bandicot. Missing keys are silently skipped so the model picker still shows
every provider whose key is available.

## Provider Fields

Each `[model.<id>]` entry can declare:

- `transport = "http"` (default) or `"apple_foundation_models"`.
- `auth_scheme = "bearer"`, `"x_api_key"`, or `"none"`.
- `capabilities = { tools = true, image_input = false }` to prevent unsupported
  tools or image inputs from reaching the provider.
- `wire_quirks.chat_max_tokens_field` as `"max_tokens"` or
  `"max_completion_tokens"` for Chat Completions.
- `wire_quirks.reasoning_response_field` as `"reasoning_content"` or
  `"reasoning"`; the same spelling is used when preserving reasoning in a
  continuation.
- `wire_quirks.send_stream_options` and `send_tool_choice` to omit optional
  request fields rejected by otherwise OpenAI-compatible servers.

Defaults preserve the existing OpenAI-compatible behavior: bearer auth,
tools and image input enabled, `max_tokens`, `reasoning_content`, and both
optional fields sent.

## Context And Compaction

The top-right `used / total` display is the estimated model-visible context, not
billing usage. `/context` shows its category breakdown. The initial value
includes the system prompt, built-in tool schemas, project instructions, enabled
skill listings, and MCP discovery metadata. MCP input schemas remain lazy behind
`search_tool` and `use_tool`; Bandicot does not inject every MCP schema at startup.

The default Bandicot profile caps OpenAI entries at
`362000` tokens and sets their per-model
`auto_compact_threshold_percent = 51`. This displays as approximately `362K`
and triggers compaction around `184620` tokens. The extra window above the
trigger is safety runway for tool-output bursts and compaction retries. Other
provider models retain their model-appropriate limits and thresholds.

Per-model values override the global `[session]` threshold:

```toml
[model.openai-sol]
context_window = 362000
auto_compact_threshold_percent = 51
```

Changing these values affects new sessions. Existing sessions retain their
persisted conversation and may require a new session before the displayed total
and startup context change completely.

## Skills And MCP Controls

- Run `/skills` to enable or disable individual skills. Disabled skills remain
  discoverable in the UI but are omitted from model-facing listings and cannot
  be invoked by the skill tool.
- Run `/mcps` to enable or disable individual MCP servers and tools.
- Use `bandicot plugin disable <name>` and `bandicot plugin enable <name>` to
  disable or enable every skill, hook, agent, and MCP contributed by one plugin.
- Use `[compat.claude]` or `[compat.cursor]` to stop importing vendor resources
  entirely. These switches require a new session:

```toml
[compat.claude]
skills = false
mcps = false

[compat.cursor]
skills = false
mcps = false
```

Canonical `.bandicot` skills are independent of vendor compatibility switches.
Use `/skills` for those. Disabling a resource does not delete its source.

## Isolation

Credential resolution is model-owned. A custom or local endpoint never borrows
an xAI session or `XAI_API_KEY`, and changing an inherited catalog entry's
`base_url` clears its inherited credential source unless the override declares
`api_key` or `env_key` itself. `auth_scheme = "none"` suppresses both
`Authorization` and `x-api-key`, even if a key is accidentally supplied by a
caller.

Generic loopback URLs are not trusted as xAI's cli-chat-proxy and do not receive
`x-grok-*` or `x-xai-*` headers. CLIProxyAPI remains isolated through the
explicit `GROK_CODEX_PROXY_TOKEN` reference in `config/codex-plan.toml`; its
Codex OAuth credential stays owned by CLIProxyAPI.

## Apple Foundation Models

Install the native-only profile on macOS 26 or later:

```sh
BANDICOT_PROFILE_SOURCE="$PWD/config/apple-foundation-models.toml" scripts/install-bandicot.sh
```

The installer builds `native/apple-foundation-models` with Swift and installs
`bandicot-apple-foundation-models` beside the Rust payload under Bandicot's
libexec directory. `scripts/build-apple-foundation-models.sh [destination]`
builds it independently. `BANDICOT_APPLE_HELPER_PREBUILT` can supply a packaged
helper, and `BANDICOT_APPLE_FOUNDATION_MODELS_HELPER` overrides its runtime path.

The transport requires macOS 26+, an eligible Apple-silicon Mac, Apple
Intelligence enabled, and the system model ready. It checks
`SystemLanguageModel.default.availability` before every generation and reports
the public unavailable reason without retrying deterministic failures.

The bridge uses versioned, 4-byte big-endian length-prefixed JSON frames over
stdio. Each request gets a dedicated helper process; cancellation drops and
kills that process. Frames and error text are bounded. FoundationModels emits
cumulative text snapshots, which the Rust transport normalizes into
non-duplicated deltas and rejects if already-emitted text is revised.

System instructions, user prompts, assistant responses, and historical tool
call/results map to FoundationModels transcript entries. The current public
FoundationModels tool API executes tools inside the Swift session and does not
provide Bandicot's pause/return external tool-call contract, so the Apple model
suppresses tools, hosted tools, and images. Dynamic JSON schema output is
supported through `GenerationSchema` for objects, arrays, primitive values,
string enums, and `anyOf`; unsupported schema constructs fail closed.
