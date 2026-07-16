<!-- Modified in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md. -->
<div align="center">

<h1>Grok Build for OpenAI</h1>

**An unofficial, source-built fork of SpaceXAI's Grok Build coding agent, with
a secret-free OpenAI Platform profile and an isolated `grok-openai` launcher.**

[OpenAI quick start](#openai-quick-start) ·
[Assumptions](#assumptions) ·
[Building from source](#building-from-source) ·
[Documentation](#documentation) ·
[Updating the fork](#updating-the-fork) ·
[Stop line](#stop-line) ·
[License](#license)

</div>

> [!IMPORTANT]
> This is a user-maintained fork. It is not affiliated with, endorsed by, or
> supported by SpaceXAI/xAI or OpenAI. `Grok Build`, `Grok`, `xAI`, and
> `OpenAI` remain their respective owners' names and marks. See
> [FORK-NOTICE.md](FORK-NOTICE.md) for the change and attribution notice.

The upstream project is SpaceXAI's terminal-based AI coding agent. This fork
retains the Rust CLI/TUI and agent runtime while adding a first-class OpenAI
Responses API distribution. The upstream source lives at
[`xai-org/grok-build`](https://github.com/xai-org/grok-build).

## OpenAI quick start

Do **not** use the xAI release installer for this fork: it installs xAI's
prebuilt binary and does not contain these OpenAI changes. Build and install
the fork itself:

```sh
./scripts/install-openai.sh
~/.local/bin/grok-openai
```

With no Platform key, the launcher starts the Grok Build TUI against a
loopback-only CLIProxyAPI Responses endpoint backed by the user's existing
Codex OAuth login. To use direct Platform billing instead, first run
`./scripts/setup-openai-key.sh`; the key setup delegates the secret prompt and
storage to macOS Keychain. The
installer creates an isolated launcher at `~/.local/bin/grok-openai`, installs
the compiled binary under `~/.local/libexec/grok-openai/`, and uses
`~/.grok-openai` for Platform mode and `~/.grok-codex-plan` for plan mode. It
does not edit `PATH`, shell startup files, terminal settings, or an existing
`~/.grok` installation. If `~/.local/bin` is not already on `PATH`, keep using
the full path shown above.

On other platforms, or for a temporary session, supply an OpenAI Platform key
through the environment and run the installer/launcher normally:

```sh
OPENAI_API_KEY="${OPENAI_API_KEY:?set it through your secret manager}" \
  ~/.local/bin/grok-openai
```

The tracked [OpenAI profile](config/openai.toml) contains no secret. Detailed
setup, model choices, security boundaries, and troubleshooting are in
[docs/OPENAI.md](docs/OPENAI.md).

## Assumptions

- ChatGPT-plan access uses the Grok Build TUI through the separately installed
  CLIProxyAPI compatibility layer on `127.0.0.1`. CLIProxyAPI owns Codex OAuth;
  the launcher reads only its protected local client token and never copies the
  OAuth credential or treats it as a Platform API key.
- In Codex-plan mode, `gpt-5.6-sol` is the default and the picker follows the
  models exposed by the local proxy. In direct Platform mode, `gpt-5.6` remains
  the floating default. Actual availability follows the authenticated account.
- macOS is the primary one-command installation target because the key helper
  uses Keychain. Source builds and environment-key launches remain available
  on Linux; Windows is still upstream best-effort.
- The fork is intentionally installed alongside any official `grok` binary.
  It does not replace, authenticate, or reconfigure the official installation.

## Building from source

Requirements:

- **Rust** — the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **protoc** — proto codegen resolves [`bin/protoc`](bin/protoc) (a
  [dotslash](https://dotslash-cli.com) launcher) or falls back to a `protoc` on
  `PATH` / `$PROTOC`.
- macOS and Linux are supported build hosts; Windows builds are best-effort
  and not currently tested from this tree.

```sh
cargo run -p xai-grok-pager-bin              # build + launch the TUI
cargo build -p xai-grok-pager-bin --release  # release binary: target/release/xai-grok-pager
cargo check -p xai-grok-pager-bin            # fast validation
```

The binary artifact is named `xai-grok-pager`; this fork's installer exposes it
as `grok-openai`. Direct Platform mode uses `OPENAI_API_KEY`. Without that key,
the launcher keeps the Grok Build TUI and selects the isolated
`~/.grok-codex-plan` profile plus CLIProxyAPI's protected local client token.

## Documentation

Fork-specific documents:

- [OpenAI setup and operation](docs/OPENAI.md)
- [One-command upstream updates](docs/UPDATING.md)
- [Fork change and attribution notice](FORK-NOTICE.md)

Full online documentation is available at
[docs.x.ai/build/overview](https://docs.x.ai/build/overview).

The user guide ships with the pager crate:
[`crates/codegen/xai-grok-pager/docs/user-guide/`](crates/codegen/xai-grok-pager/docs/user-guide/)
— getting started, keyboard shortcuts, slash commands, configuration, theming,
MCP servers, skills, plugins, hooks, headless mode, sandboxing, and more.

## Repository layout

| Path | Contents |
|------|----------|
| `crates/codegen/xai-grok-pager-bin` | Composition-root package; builds the `xai-grok-pager` binary |
| `crates/codegen/xai-grok-pager` | The TUI: scrollback, prompt, modals, rendering |
| `crates/codegen/xai-grok-shell` | Agent runtime + leader/stdio/headless entry points |
| `crates/codegen/xai-grok-tools` | Tool implementations (terminal, file edit, search, ...) |
| `crates/codegen/xai-grok-workspace` | Host filesystem, VCS, execution, checkpoints |
| `crates/codegen/...` | The rest of the CLI crate closure (config, MCP, markdown, sandbox, ...) |
| `crates/common/`, `crates/build/`, `prod/mc/` | Small shared leaf crates pulled in by the closure |
| `third_party/` | Vendored upstream source (Mermaid diagram stack) — see below |

> [!IMPORTANT]
> The root `Cargo.toml` (workspace members, dependency versions, lints,
> profiles) is **generated** — treat it as read-only. Prefer editing per-crate
> `Cargo.toml` files.

## Development

```sh
cargo check -p <crate>        # always target specific crates; full-workspace builds are slow
cargo test -p xai-grok-config # per-crate tests
cargo clippy -p <crate>       # lint config: clippy.toml at the repo root
cargo fmt --all               # rustfmt.toml at the repo root
```

## Updating the fork

From a clean `main` branch, the complete update is one command:

```sh
./scripts/update-from-upstream.sh
```

The updater fetches `origin` and `upstream`, integrates upstream in an isolated
candidate worktree, runs the fork's checks and release build, and only then
pushes the tested candidate to `origin/main` and fast-forwards local `main` to
that same commit. A tracked upstream-snapshot marker supports an explicitly
accepted append-only bridge if upstream force-rewrites, rebases, rewinds, or
rolls back history; the refusal prints an exact
`--accept-upstream-rewrite=<previous-sha>..<fetched-sha>` pin to use only after
inspecting that unexpected history change. The updater never force-pushes,
resets, rebases, stashes, or pushes to upstream. See
[docs/UPDATING.md](docs/UPDATING.md) for prerequisites and failure recovery.

## Stop line

This fork's implementation is considered complete when all of the following
are true: the OpenAI profile parses, provider-isolation and Responses API tests
pass, a release `grok-openai` binary builds, a keyless/mock end-to-end prompt
passes without contacting xAI, install/update scripts pass their fail-closed
tests, and the exact tested commit is published to the fork. A live paid API
request is a separate account-acceptance check because it requires the user's
private key and project quota; it must never be silently substituted with a
ChatGPT/Codex session token.

## Contributing

> [!NOTE]
> External contributions are not accepted. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

First-party code in this repository is licensed under the **Apache License,
Version 2.0** — see [`LICENSE`](LICENSE).

Third-party and vendored code remains under its original licenses. See:

- [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) — crates.io / git dependencies,
  bundled UI themes, and **in-tree source ports** (including openai/codex and
  sst/opencode tool implementations)
- [`crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md`](crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md)
  — crate-local notice for the codex and opencode ports (license texts +
  Apache §4(b) change notice)
- [`third_party/NOTICE`](third_party/NOTICE) — vendored Mermaid-stack index
