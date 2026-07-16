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
./scripts/setup-openai-key.sh
./scripts/install-openai.sh
~/.local/bin/grok-openai
```

The key setup delegates the secret prompt and storage to macOS Keychain. The
installer creates an isolated launcher at `~/.local/bin/grok-openai`, installs
the compiled binary under `~/.local/libexec/grok-openai/`, and uses
`~/.grok-openai` as `GROK_HOME`. It does not edit `PATH`, shell startup files,
terminal settings, or an existing `~/.grok` installation. If `~/.local/bin`
is not already on `PATH`, keep using the full path shown above.

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

- "OpenAI account" means an OpenAI Platform project with API access, billing,
  and an API key. A ChatGPT subscription or Codex sign-in does not by itself
  grant Platform API access, and this fork never copies Codex OAuth tokens.
- "Latest models" means the floating `gpt-5.6` alias by default, plus curated
  `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, and `gpt-5.3-codex`
  selections. Availability still depends on the user's OpenAI project.
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
as `grok-openai`. The fork does not use the upstream browser login for OpenAI:
it requires `OPENAI_API_KEY`, supplied by the environment or its isolated
Keychain-backed launcher.

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
that same commit. It never force-pushes or pushes to upstream. See
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
