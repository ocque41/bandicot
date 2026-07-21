#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)
. "$SCRIPT_DIR/lib/openai-workflow.sh"

case ${1:-} in
    '') ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/install-bandicot.sh

Build, validate, and install Bandicot. The installer does not edit shell
startup files and never reads, writes, or removes ~/.grok.

Test/packaging overrides:
  BANDICOT_PREBUILT          executable binary to install instead of building
  BANDICOT_PROFILE_SOURCE    profile TOML (default: config/openai.toml)
  BANDICOT_HOME              active profile/state dir (default: ~/.bandicot)
  BANDICOT_BIN_DIR           launcher dir (default: ~/.local/bin)
  BANDICOT_LIBEXEC_DIR       binary dir (default: ~/.local/libexec/bandicot)
  BANDICOT_APPLE_HELPER_PREBUILT  prebuilt native helper (optional)
  BANDICOT_NATIVE_ONLY_PROFILE    skip network credential lookup (0 or 1)
  BANDICOT_PROXY_TOKEN_FILE  CLIProxyAPI client token
  BANDICOT_OPENCODE_GO_KEYCHAIN_SERVICE  OpenCode Go Keychain service
  BANDICOT_OPENCODE_GO_KEYCHAIN_ACCOUNT  OpenCode Go Keychain account
EOF
        exit 0
        ;;
    *) openai_workflow_die "unknown argument: $1" ;;
esac

[ -n "${HOME:-}" ] || openai_workflow_die 'HOME is not set'
BANDICOT_HOME=${BANDICOT_HOME:-$HOME/.bandicot}
BANDICOT_BIN_DIR=${BANDICOT_BIN_DIR:-$HOME/.local/bin}
BANDICOT_LIBEXEC_DIR=${BANDICOT_LIBEXEC_DIR:-$HOME/.local/libexec/bandicot}
PROFILE_SOURCE=${BANDICOT_PROFILE_SOURCE:-$REPO_ROOT/config/openai.toml}
PROXY_TOKEN_FILE=${BANDICOT_PROXY_TOKEN_FILE:-$HOME/.cli-proxy-api/client-token}
KEYCHAIN_SERVICE=${BANDICOT_KEYCHAIN_SERVICE:-bandicot.openai}
LEGACY_KEYCHAIN_SERVICE=ocque41.grok-build.openai
KEYCHAIN_ACCOUNT=${BANDICOT_KEYCHAIN_ACCOUNT:-openai-platform}
OPENCODE_GO_KEYCHAIN_SERVICE=${BANDICOT_OPENCODE_GO_KEYCHAIN_SERVICE:-bandicot.opencode-go}
OPENCODE_GO_KEYCHAIN_ACCOUNT=${BANDICOT_OPENCODE_GO_KEYCHAIN_ACCOUNT:-opencode-go}

case "$BANDICOT_HOME$BANDICOT_BIN_DIR$BANDICOT_LIBEXEC_DIR$PROXY_TOKEN_FILE" in
    *'
'*) openai_workflow_die 'install paths must not contain newline characters' ;;
esac
for bandicot_path in "$HOME" "$BANDICOT_HOME" "$BANDICOT_BIN_DIR" "$BANDICOT_LIBEXEC_DIR" "$PROXY_TOKEN_FILE"; do
    case $bandicot_path in
        /*) ;;
        *) openai_workflow_die 'HOME and install paths must be absolute' ;;
    esac
done
case $BANDICOT_HOME in
    */../*|*/..|*/./*|*/.) openai_workflow_die 'BANDICOT_HOME must not contain . or .. path components' ;;
esac
case "$BANDICOT_HOME/" in
    "$HOME/.grok/"|"$HOME/.grok/"*) openai_workflow_die 'refusing to use or modify the legacy ~/.grok directory' ;;
esac
[ ! -L "$BANDICOT_HOME" ] || openai_workflow_die 'BANDICOT_HOME must not be a symbolic link'
[ -f "$PROFILE_SOURCE" ] || openai_workflow_die "Bandicot profile not found: $PROFILE_SOURCE"
NATIVE_ONLY_PROFILE=${BANDICOT_NATIVE_ONLY_PROFILE:-0}
if grep -q '^# BANDICOT_NATIVE_ONLY_PROFILE=1$' "$PROFILE_SOURCE"; then NATIVE_ONLY_PROFILE=1; fi
case $NATIVE_ONLY_PROFILE in 0|1) ;; *) openai_workflow_die 'BANDICOT_NATIVE_ONLY_PROFILE must be 0 or 1' ;; esac

if [ -n "${BANDICOT_PREBUILT:-}" ]; then
    BINARY=$(openai_workflow_abspath "$BANDICOT_PREBUILT") || \
        openai_workflow_die "cannot resolve prebuilt binary: $BANDICOT_PREBUILT"
else
    CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
    case $CARGO_TARGET_DIR in /*) ;; *) CARGO_TARGET_DIR=$REPO_ROOT/$CARGO_TARGET_DIR ;; esac
    export CARGO_TARGET_DIR
    "$SCRIPT_DIR/validate-openai.sh"
    BINARY=$CARGO_TARGET_DIR/release-dist/xai-grok-pager
fi
[ -f "$BINARY" ] && [ -x "$BINARY" ] || openai_workflow_die "binary is not executable: $BINARY"
"$BINARY" --version >/dev/null || openai_workflow_die 'candidate binary failed its --version smoke test'
BINARY_VERSION=$("$BINARY" --version 2>&1 | sed -n '1p')

APPLE_HELPER=
if [ -n "${BANDICOT_APPLE_HELPER_PREBUILT:-}" ]; then
    APPLE_HELPER=$(openai_workflow_abspath "$BANDICOT_APPLE_HELPER_PREBUILT") || \
        openai_workflow_die "cannot resolve prebuilt Apple helper: $BANDICOT_APPLE_HELPER_PREBUILT"
elif [ "$(uname -s)" = Darwin ] && [ "$NATIVE_ONLY_PROFILE" -eq 1 ]; then
    APPLE_HELPER=$REPO_ROOT/target/apple-foundation-models/bandicot-apple-foundation-models
    "$SCRIPT_DIR/build-apple-foundation-models.sh" "$APPLE_HELPER" >/dev/null
elif [ "$NATIVE_ONLY_PROFILE" -eq 1 ]; then
    openai_workflow_die 'the Apple Foundation Models profile requires macOS 26 or later and the native helper'
fi
if [ -n "$APPLE_HELPER" ]; then
    [ -f "$APPLE_HELPER" ] && [ -x "$APPLE_HELPER" ] || openai_workflow_die "Apple helper is not executable: $APPLE_HELPER"
fi

migrate_legacy_home() {
    _mlh_source=$1
    [ ! -e "$BANDICOT_HOME" ] || return 1
    [ -d "$_mlh_source" ] && [ ! -L "$_mlh_source" ] || return 1
    _mlh_stage=$BANDICOT_HOME.migrate.$$
    rm -rf "$_mlh_stage"
    mkdir -p "$_mlh_stage"
    cp -Rp "$_mlh_source/." "$_mlh_stage/" || openai_workflow_die "failed to copy legacy Bandicot data from $_mlh_source"
    diff -qr "$_mlh_source" "$_mlh_stage" >/dev/null || openai_workflow_die "legacy Bandicot data verification failed for $_mlh_source"
    mv "$_mlh_stage" "$BANDICOT_HOME" || openai_workflow_die "failed to activate migrated Bandicot data"
    openai_workflow_note "Migrated and verified Bandicot data from $_mlh_source to $BANDICOT_HOME"
    return 0
}

if [ ! -e "$BANDICOT_HOME" ]; then
    migrate_legacy_home "$HOME/.grok-openai" || \
        migrate_legacy_home "$HOME/.grok-codex-plan" || true
fi
mkdir -p "$BANDICOT_HOME" "$BANDICOT_BIN_DIR" "$BANDICOT_LIBEXEC_DIR"
chmod 700 "$BANDICOT_HOME"

BINARY_DEST=$BANDICOT_LIBEXEC_DIR/bandicot
APPLE_HELPER_DEST=$BANDICOT_LIBEXEC_DIR/bandicot-apple-foundation-models
CONFIG_DEST=$BANDICOT_HOME/config.toml
CONFIG_HASH_DEST=$BANDICOT_HOME/.installed-config.sha256
CANONICAL_CONFIG_DEST=$BANDICOT_HOME/.canonical-config.toml
LAUNCHER_DEST=$BANDICOT_BIN_DIR/bandicot
[ ! -L "$CONFIG_DEST" ] || openai_workflow_die 'refusing a symbolic-link runtime config'

openai_workflow_atomic_copy "$BINARY" "$BINARY_DEST" 755 || openai_workflow_die "failed to install binary at $BINARY_DEST"
if [ -n "$APPLE_HELPER" ]; then
    openai_workflow_atomic_copy "$APPLE_HELPER" "$APPLE_HELPER_DEST" 755 || openai_workflow_die "failed to install Apple helper at $APPLE_HELPER_DEST"
fi
openai_workflow_atomic_copy "$PROFILE_SOURCE" "$CANONICAL_CONFIG_DEST" 600 || openai_workflow_die 'failed to install canonical profile'

new_hash=$(openai_workflow_hash_file "$PROFILE_SOURCE")
install_profile=0
if [ ! -e "$CONFIG_DEST" ]; then
    install_profile=1
elif [ -f "$CONFIG_HASH_DEST" ]; then
    recorded_hash=$(sed -n '1p' "$CONFIG_HASH_DEST" 2>/dev/null || true)
    current_hash=$(openai_workflow_hash_file "$CONFIG_DEST" 2>/dev/null || true)
    if [ -n "$recorded_hash" ] && [ "$recorded_hash" = "$current_hash" ]; then
        install_profile=1
    elif [ "$current_hash" = "$new_hash" ]; then
        install_profile=1
    fi
fi
if [ "$install_profile" -eq 1 ]; then
    openai_workflow_atomic_copy "$PROFILE_SOURCE" "$CONFIG_DEST" 600 || openai_workflow_die 'failed to install profile'
    openai_workflow_atomic_text "$CONFIG_HASH_DEST" 600 "$new_hash" || openai_workflow_die 'failed to record installed profile hash'
else
    openai_workflow_note "Preserved customized profile: $CONFIG_DEST"
    openai_workflow_note "The new canonical profile is available at: $CANONICAL_CONFIG_DEST"
fi

DEFAULT_HOME_QUOTED=$(openai_workflow_shell_quote "$BANDICOT_HOME")
DEFAULT_LIBEXEC_QUOTED=$(openai_workflow_shell_quote "$BANDICOT_LIBEXEC_DIR")
DEFAULT_SERVICE_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_SERVICE")
LEGACY_SERVICE_QUOTED=$(openai_workflow_shell_quote "$LEGACY_KEYCHAIN_SERVICE")
DEFAULT_ACCOUNT_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_ACCOUNT")
DEFAULT_PROXY_TOKEN_FILE_QUOTED=$(openai_workflow_shell_quote "$PROXY_TOKEN_FILE")
OPENCODE_GO_SERVICE_QUOTED=$(openai_workflow_shell_quote "$OPENCODE_GO_KEYCHAIN_SERVICE")
OPENCODE_GO_ACCOUNT_QUOTED=$(openai_workflow_shell_quote "$OPENCODE_GO_KEYCHAIN_ACCOUNT")

LAUNCHER_TEMP=$(mktemp "${LAUNCHER_DEST}.tmp.XXXXXX") || openai_workflow_die 'failed to stage launcher'
cat >"$LAUNCHER_TEMP" <<EOF
#!/bin/sh
# BANDICOT_INSTALLER_PROVENANCE=bandicot-local-v1
set -eu

BANDICOT_HOME=\${BANDICOT_HOME:-$DEFAULT_HOME_QUOTED}
BANDICOT_LIBEXEC_DIR=\${BANDICOT_LIBEXEC_DIR:-$DEFAULT_LIBEXEC_QUOTED}
BANDICOT_KEYCHAIN_SERVICE=\${BANDICOT_KEYCHAIN_SERVICE:-$DEFAULT_SERVICE_QUOTED}
BANDICOT_LEGACY_KEYCHAIN_SERVICE=\${BANDICOT_LEGACY_KEYCHAIN_SERVICE:-$LEGACY_SERVICE_QUOTED}
BANDICOT_KEYCHAIN_ACCOUNT=\${BANDICOT_KEYCHAIN_ACCOUNT:-$DEFAULT_ACCOUNT_QUOTED}
BANDICOT_PROXY_TOKEN_FILE=\${BANDICOT_PROXY_TOKEN_FILE:-$DEFAULT_PROXY_TOKEN_FILE_QUOTED}
BANDICOT_OPENCODE_GO_KEYCHAIN_SERVICE=\${BANDICOT_OPENCODE_GO_KEYCHAIN_SERVICE:-$OPENCODE_GO_SERVICE_QUOTED}
BANDICOT_OPENCODE_GO_KEYCHAIN_ACCOUNT=\${BANDICOT_OPENCODE_GO_KEYCHAIN_ACCOUNT:-$OPENCODE_GO_ACCOUNT_QUOTED}
BANDICOT_APPLE_FOUNDATION_MODELS_HELPER=\${BANDICOT_APPLE_FOUNDATION_MODELS_HELPER:-\$BANDICOT_LIBEXEC_DIR/bandicot-apple-foundation-models}
BANDICOT_NATIVE_ONLY_PROFILE=\${BANDICOT_NATIVE_ONLY_PROFILE:-$NATIVE_ONLY_PROFILE}
BANDICOT_CLIPROXYAPI=\${BANDICOT_CLIPROXYAPI:-cliproxyapi}

[ -n "\${HOME:-}" ] || { printf '%s\n' 'error: HOME is not set.' >&2; exit 1; }
case \$BANDICOT_HOME in /*) ;; *) printf '%s\n' 'error: BANDICOT_HOME must be absolute.' >&2; exit 1 ;; esac
case \$BANDICOT_HOME in */../*|*/..|*/./*|*/.) printf '%s\n' 'error: BANDICOT_HOME must not contain . or .. path components.' >&2; exit 1 ;; esac
case "\$BANDICOT_HOME/" in "\$HOME/.grok/"|"\$HOME/.grok/"*) printf '%s\n' 'error: refusing to use the legacy ~/.grok directory.' >&2; exit 1 ;; esac
[ ! -L "\$BANDICOT_HOME" ] || { printf '%s\n' 'error: BANDICOT_HOME must not be a symbolic link.' >&2; exit 1; }

export GROK_HOME=\$BANDICOT_HOME
export BANDICOT_APPLE_FOUNDATION_MODELS_HELPER
export GROK_DISABLE_AUTOUPDATER=1 GROK_OPENAI_DISABLE_VENDOR_UPDATE=1
export GROK_IMAGE_GEN=0 GROK_IMAGE_EDIT=0 GROK_VOICE_MODE=0 GROK_MANAGED_CONFIG=0 GROK_DISABLE_CUSTOM_BRIDGE=1
unset XAI_API_KEY GROK_CODE_XAI_API_KEY GROK_AUTH GROK_AUTH_PATH GROK_AUTH_PROVIDER_COMMAND GROK_AUTH_PROVIDER_LABEL 2>/dev/null || true

case \${1:-} in update) printf '%s\n' 'error: the vendor updater is disabled for Bandicot.' >&2; exit 2 ;; esac
bandicot_login() {
    command -v "\$BANDICOT_CLIPROXYAPI" >/dev/null 2>&1 || { printf '%s\n' 'error: CLIProxyAPI is required for ChatGPT sign-in.' >&2; return 1; }
    case \${1:-} in ''|--browser) "\$BANDICOT_CLIPROXYAPI" -codex-login ;; --device-code) "\$BANDICOT_CLIPROXYAPI" -codex-device-login ;; *) printf 'error: unknown login option: %s\n' "\$1" >&2; return 2 ;; esac
}
if [ "\${1:-}" = login ]; then shift; bandicot_login "\${1:-}"; exit \$?; fi

if [ "\$BANDICOT_NATIVE_ONLY_PROFILE" = 1 ]; then
    unset OPENAI_API_KEY BANDICOT_OPENAI_TOKEN XAI_API_KEY GROK_CODE_XAI_API_KEY 2>/dev/null || true
    exec "\$BANDICOT_LIBEXEC_DIR/bandicot" --no-auto-update "\$@"
fi

needs_key=1
case \${1:-} in -h|--help|-v|-V|--version|leader|models|completions) needs_key=0 ;; esac

# Load all provider keys simultaneously. Each provider resolves its own
# credential independently; missing keys are silently skipped so the
# model picker can still offer every provider whose key is available.

# --- OpenCode Go ---
if [ -z "\${OPENCODE_GO_API_KEY:-}" ]; then
    if [ -f "\$BANDICOT_HOME/opencode-go.key" ] && [ ! -L "\$BANDICOT_HOME/opencode-go.key" ]; then
        OPENCODE_GO_API_KEY=\$(cat "\$BANDICOT_HOME/opencode-go.key" 2>/dev/null || true)
        export OPENCODE_GO_API_KEY
    elif [ -x /usr/bin/security ]; then
        OPENCODE_GO_API_KEY=\$(/usr/bin/security find-generic-password -a "\$BANDICOT_OPENCODE_GO_KEYCHAIN_ACCOUNT" -s "\$BANDICOT_OPENCODE_GO_KEYCHAIN_SERVICE" -w 2>/dev/null || true)
        export OPENCODE_GO_API_KEY
    fi
fi

# --- OpenAI Platform ---
if [ \$needs_key -eq 1 ] && [ -z "\${OPENAI_API_KEY:-}" ]; then
    if [ -f "\$BANDICOT_HOME/openai.key" ] && [ ! -L "\$BANDICOT_HOME/openai.key" ]; then
        OPENAI_API_KEY=\$(cat "\$BANDICOT_HOME/openai.key" 2>/dev/null || true)
        export OPENAI_API_KEY
    elif [ -x /usr/bin/security ]; then
        OPENAI_API_KEY=\$(/usr/bin/security find-generic-password -a "\$BANDICOT_KEYCHAIN_ACCOUNT" -s "\$BANDICOT_KEYCHAIN_SERVICE" -w 2>/dev/null || true)
        if [ -z "\$OPENAI_API_KEY" ] && [ "\$BANDICOT_KEYCHAIN_SERVICE" != "\$BANDICOT_LEGACY_KEYCHAIN_SERVICE" ]; then
            OPENAI_API_KEY=\$(/usr/bin/security find-generic-password -a "\$BANDICOT_KEYCHAIN_ACCOUNT" -s "\$BANDICOT_LEGACY_KEYCHAIN_SERVICE" -w 2>/dev/null || true)
        fi
        export OPENAI_API_KEY
    fi
fi
if [ -n "\${OPENAI_API_KEY:-}" ]; then
    export BANDICOT_OPENAI_BASE_URL=https://api.openai.com/v1 BANDICOT_OPENAI_TOKEN=\$OPENAI_API_KEY
fi

# Exec without key check for commands that don't need authentication.
if [ \$needs_key -eq 0 ]; then
    exec "\$BANDICOT_LIBEXEC_DIR/bandicot" --no-auto-update "\$@"
fi

# Exec if any provider key was loaded successfully.
_bandicot_any_key=0
[ -n "\${OPENCODE_GO_API_KEY:-}" ] && _bandicot_any_key=1
[ -n "\${OPENAI_API_KEY:-}" ] && _bandicot_any_key=1
if [ \$_bandicot_any_key -eq 1 ]; then
    exec "\$BANDICOT_LIBEXEC_DIR/bandicot" --no-auto-update "\$@"
fi

# Fallback: try the local CLIProxyAPI ChatGPT proxy if no Platform key is set.
if [ -f "\$BANDICOT_PROXY_TOKEN_FILE" ] && [ ! -L "\$BANDICOT_PROXY_TOKEN_FILE" ]; then
    token_mode=\$(stat -f '%Lp' "\$BANDICOT_PROXY_TOKEN_FILE" 2>/dev/null || stat -c '%a' "\$BANDICOT_PROXY_TOKEN_FILE" 2>/dev/null || true)
    case \$token_mode in 400|600) ;; *) printf '%s\n' 'error: CLIProxyAPI client token must have mode 0400 or 0600.' >&2; exit 1 ;; esac
    IFS= read -r BANDICOT_OPENAI_TOKEN <"\$BANDICOT_PROXY_TOKEN_FILE" || true
    if [ -n "\$BANDICOT_OPENAI_TOKEN" ]; then
        export BANDICOT_OPENAI_TOKEN BANDICOT_OPENAI_BASE_URL=http://127.0.0.1:8317/v1
        printf '%s\n' 'Bandicot: using your ChatGPT account through local CLIProxyAPI.' >&2
        exec "\$BANDICOT_LIBEXEC_DIR/bandicot" --no-auto-update "\$@"
    fi
fi
if [ -t 0 ] && [ -t 2 ]; then
    printf '%s' 'Bandicot needs ChatGPT access. Sign in now? [Y/n] ' >&2
    IFS= read -r answer || answer=n
    case \$answer in ''|y|Y|yes|YES|Yes) bandicot_login --browser || exit \$? ;; *) printf '%s\n' 'Sign-in cancelled. Run bandicot login when ready.' >&2; exit 1 ;; esac
fi
printf '%s\n' 'error: no provider API key or protected CLIProxyAPI client token is available.' >&2
printf '%s\n' 'Run bandicot login, or one of:' >&2
printf '%s\n' '  scripts/setup-openai-key.sh        (OpenAI Platform)' >&2
printf '%s\n' '  scripts/setup-opencode-go-key.sh   (OpenCode Go)' >&2
exit 1
EOF
chmod 755 "$LAUNCHER_TEMP"
/bin/sh -n "$LAUNCHER_TEMP" || { rm -f "$LAUNCHER_TEMP"; openai_workflow_die 'generated launcher failed shell syntax validation'; }
mv -f "$LAUNCHER_TEMP" "$LAUNCHER_DEST" || openai_workflow_die "failed to install launcher at $LAUNCHER_DEST"
"$LAUNCHER_DEST" --version >/dev/null || openai_workflow_die 'installed launcher failed its --version smoke test'

openai_workflow_note "Installed Bandicot: $LAUNCHER_DEST"
openai_workflow_note "Installed version: $BINARY_VERSION"
openai_workflow_note "Active profile and state: $BANDICOT_HOME"
case :$PATH: in *:"$BANDICOT_BIN_DIR":*) ;; *) openai_workflow_note "Launch it by absolute path: $LAUNCHER_DEST" ;; esac
