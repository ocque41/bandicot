# Bandicot OpenAI and ChatGPT setup

This document covers the unofficial fork's OpenAI-only distribution. It uses
the OpenAI Responses API through the tracked, secret-free
[`config/openai.toml`](../config/openai.toml) profile and installs alongside any
official Grok Build installation.

## Assumptions and boundaries

- An **OpenAI account** here means an OpenAI Platform project that can create
  API keys, has billing or credits, and is permitted to use the selected model.
  ChatGPT Plus/Pro/Business and Codex sign-in are separate products and do not
  automatically provide Platform API quota.
- Direct Platform inference uses `OPENAI_API_KEY`. When it is absent, the Grok
  Build runtime can instead use a loopback CLIProxyAPI Responses endpoint.
  CLIProxyAPI owns and refreshes Codex OAuth; this launcher reads only the
  proxy's protected local client token and never reads a ChatGPT cookie or
  Codex OAuth token.
- The supported wire protocol is the OpenAI **Responses API**. The curated
  profile intentionally does not use Chat Completions because the agent relies
  on Responses-style reasoning and function-call events.
- `gpt-5.6` is a floating alias. OpenAI may move that alias to a newer snapshot
  without a fork update. Use `openai-sol`, `openai-terra`, or `openai-luna` when
  a stable model family member is preferable.
- Model availability, billing limits, data controls, and regional restrictions
  belong to the user's OpenAI project. The fork cannot grant access or bypass an
  account restriction.
- macOS is the primary install target. Linux can use the same source-built
  launcher with an externally supplied environment key; the Keychain helper is
  macOS-specific. No script modifies Terminal, Ghostty, tmux, shell startup
  files, macOS privacy settings, Homebrew, or Google Cloud SDK configuration.

## Prerequisites

The installer builds from source, so the host needs:

- the Rust toolchain selected by `rust-toolchain.toml` (normally managed by
  `rustup`);
- `protoc`, available through this repository's `bin/protoc` dotslash launcher
  or through `PATH`/`PROTOC`;
- Git and a clean checkout of this fork;
- on macOS, `/usr/bin/security` for optional Keychain-backed secret storage.

An OpenAI Platform key can be created in the OpenAI Platform dashboard. Treat
it as a password: never commit it, paste it into TOML, include it in an issue,
or pass it as a command-line argument.

## Install on macOS

From the repository root:

```sh
./scripts/install-bandicot.sh
~/.local/bin/bandicot --version
~/.local/bin/bandicot
```

The final command selects one of two supported modes:

- with `OPENAI_API_KEY` (environment or Keychain), it starts this fork's Grok
  Build TUI against the OpenAI Responses API;
- without a Platform key, it starts the same Grok Build TUI using
  `config/codex-plan.toml`, CLIProxyAPI on `127.0.0.1:8317`, and the protected
  `~/.cli-proxy-api/client-token`. CLIProxyAPI must already be running and have
  a valid credential. Run `bandicot login` for browser OAuth or
  `bandicot login --device-code` for device-code authentication. On first
  interactive launch without credentials, Bandicot offers to start browser login.

Run `./scripts/setup-openai-key.sh` before launching when Grok Build mode is
required.

`setup-openai-key.sh` invokes `/usr/bin/security` so Keychain performs the
interactive secret prompt. The script does not read the key itself and places
no key in an argument, configuration file, log, or repository file. The
Keychain service name is `ocque41.grok-build.openai`.

`install-openai.sh` builds the release binary and installs an isolated layout:

| Item | Default path |
|---|---|
| Launcher | `~/.local/bin/bandicot` |
| Compiled binary | `~/.local/libexec/grok-openai/bandicot` |
| Runtime home | `~/.grok-openai` |
| Runtime profile | `~/.grok-openai/config.toml` |

On later installs, the runtime profile is upgraded automatically only when it
still matches the previously installed canonical profile. A user-edited
`config.toml` is preserved; the new canonical copy is written to
`~/.local/libexec/grok-openai/openai.toml` for explicit review and merging.

The installer does not add `~/.local/bin` to `PATH`. The full launcher path
always works; if the directory is already on `PATH`, `bandicot` is enough.
The install destinations are overrideable by the script's documented
environment variables, which the test suite uses to keep acceptance runs
inside temporary directories.

## Environment-key launch

The launcher gives an already-set, non-empty `OPENAI_API_KEY` priority over
Keychain. This is useful for CI, Linux, or a temporary project credential:

```sh
OPENAI_API_KEY="${OPENAI_API_KEY:?set it through your secret manager}" \
  ~/.local/bin/bandicot
```

Do not add the key to `.env`, `.envrc`, `config/openai.toml`, shell startup
files, or a checked-in CI definition. Use the CI provider's masked secret store
or another process-level secret injector.

If no Platform key exists, the launcher accepts the CLIProxyAPI client token
only from a regular, non-symlink file with mode `0400` or `0600`. It exposes
that value only as `GROK_CODEX_PROXY_TOKEN` to the child Grok Build process and
selects the loopback-only profile. It never falls back to a cached xAI session
or `XAI_API_KEY`.

## Models

The model picker uses fork-local names while requests send the OpenAI model IDs:

| Select with `-m` or `/model` | OpenAI model | Context | Maximum output | Intended use |
|---|---|---:|---:|---|
| `openai-latest` | `gpt-5.6` | 1,050,000 | 128,000 | Default floating latest alias |
| `openai-sol` | `gpt-5.6-sol` | 1,050,000 | 128,000 | Highest-capability coding/reasoning |
| `openai-terra` | `gpt-5.6-terra` | 1,050,000 | 128,000 | Capability/cost balance |
| `openai-luna` | `gpt-5.6-luna` | 1,050,000 | 128,000 | Cost-sensitive and auxiliary work |
| `openai-codex` | `gpt-5.3-codex` | 400,000 | 128,000 | Specialized coding compatibility |

The curated metadata is reviewed against OpenAI's
[latest-model guide](https://developers.openai.com/api/docs/guides/latest-model),
[GPT-5.6 Sol model page](https://developers.openai.com/api/docs/models/gpt-5.6-sol),
and [Responses API reference](https://developers.openai.com/api/reference/resources/responses/methods/create).
The [`/v1/models` list](https://developers.openai.com/api/reference/resources/models/methods/list)
identifies models available to a key but does not replace the capability and
context metadata in this profile.

Examples:

```sh
~/.local/bin/bandicot -m openai-sol
~/.local/bin/bandicot -m openai-terra -p "Review this repository"
```

All entries use `api_backend = "responses"` and `agent_type = "codex"`. The
GPT-5.6 family exposes `none`, `low`, `medium`, `high`, `xhigh`, and the literal
Responses API `max` tier. GPT-5.3 Codex exposes `low`, `medium`, `high`, and
`xhigh`. The default is the cost-safe official `medium` tier. Temperature and
`top_p` are omitted so OpenAI applies model-appropriate defaults.

The default, session-summary, image-description, prompt-suggestion, and
web-search auxiliary slots are all explicitly pinned to an OpenAI profile.
This prevents an internal task from silently choosing a compiled xAI model.

## Deliberately disabled service surfaces

The OpenAI profile disables the upstream auto-updater, telemetry, feedback,
trace upload, Mixpanel, external OpenTelemetry export, Sentry-style error
reporting, managed-config sync, remote xAI model/settings fetches, video
generation, and hosted backend tools.

The launcher also sets `GROK_VOICE_MODE=0` and disables xAI-only
image/edit/video/voice surfaces. Provider
guards prevent a non-xAI model credential from being sent to an xAI media
endpoint. OpenAI hosted web search remains disabled until its provider-specific
wire shape and tool lifecycle have a dedicated isolation acceptance test.
Local filesystem, terminal, code-search, and configured MCP tools are separate
and continue to work subject to normal permission prompts.

## Verify without spending API quota

The repository acceptance suite uses a local mock Responses server. It checks
the request path and body, Bearer authentication, tool-call round trips,
streaming terminal events, missing-key fail-closed behavior, absence of xAI
headers/tools on OpenAI requests, and a built-binary headless prompt. It does
not need a real API key or network request to OpenAI.

Run the focused verification command documented by the installer/update
scripts, or run the relevant Rust packages directly during development. A live
account acceptance check is optional and separate:

```sh
~/.local/bin/grok-openai -m openai-luna -p "Reply with exactly: OPENAI_OK"
```

That command incurs normal OpenAI usage and succeeds only if the private key,
project quota, model access, and network path are valid. Never report it as
passed unless it was actually run with the user's key.

## Troubleshooting

### `OPENAI_API_KEY` is missing

On macOS, rerun `./scripts/setup-openai-key.sh`. For environment-key launches,
confirm the secret injector exports a non-empty value in the launcher process.
Do not work around the error with `XAI_API_KEY` or `grok login`; those are a
different provider.

### `401` or `invalid_api_key`

The key reached OpenAI but was rejected. Replace/revoke it in the Platform
dashboard, run `./scripts/setup-openai-key.sh --replace`, and verify it belongs
to the intended project. Logs intentionally redact credentials; do not turn on
shell tracing around secret setup.

### `403`, model unavailable, or project restriction

Select another curated model (for example, `openai-luna`) or ask the OpenAI
project administrator to grant access. `GET /v1/models` is only an inventory of
IDs; it does not provide reliable context-window or tool-capability metadata,
which is why the fork ships a reviewed static profile.

### `429`

The project reached a rate, token, or spending limit. Wait for the limit window
or change the project's OpenAI limits. Reinstalling the fork cannot change an
account quota.

### The launcher is not found

Run `~/.local/bin/grok-openai`. The installer intentionally does not edit
`PATH` or any shell startup file.

### Build prerequisites are missing

Install Rust/protoc using the host's normal, user-approved workflow, then rerun
the installer. This repository does not reconfigure Homebrew or shell startup
files on the user's behalf.

## Updating

Use one command from a clean `main` checkout:

```sh
./scripts/update-from-upstream.sh
```

See [UPDATING.md](UPDATING.md) for the transactional update flow and recovery.

## Stop line

The implementation goal is achieved at the exact fork commit where:

1. the secret-free profile and documented model metadata parse;
2. provider-isolation and Responses streaming/tool-call tests pass;
3. the release binary builds and a local mock end-to-end prompt succeeds;
4. install and update scripts pass their fail-closed tests; and
5. that tested commit is pushed to the user's fork.

A live OpenAI call is account acceptance, not a reason to keep changing code
or scanning indefinitely. If no private key is supplied, the honest final gate
is "not run", with the one-line command above left for the account owner.
