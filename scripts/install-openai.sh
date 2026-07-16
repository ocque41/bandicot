#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)
# shellcheck source=scripts/lib/openai-workflow.sh
. "$SCRIPT_DIR/lib/openai-workflow.sh"

case ${1:-} in
    '' ) ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/install-openai.sh

Build, validate, and install the OpenAI-specific launcher. The installer never
edits shell startup files and never reads or writes ~/.grok.

Test/packaging overrides:
  GROK_OPENAI_PREBUILT         executable binary to install instead of building
  GROK_OPENAI_PROFILE_SOURCE  profile TOML (default: config/openai.toml)
  GROK_OPENAI_HOME            isolated state dir (default: ~/.grok-openai)
  GROK_OPENAI_BIN_DIR         launcher dir (default: ~/.local/bin)
  GROK_OPENAI_LIBEXEC_DIR     binary dir (default: ~/.local/libexec/grok-openai)
EOF
        exit 0
        ;;
    *) openai_workflow_die "unknown argument: $1" ;;
esac

[ -n "${HOME:-}" ] || openai_workflow_die 'HOME is not set'

GROK_OPENAI_HOME=${GROK_OPENAI_HOME:-$HOME/.grok-openai}
GROK_OPENAI_BIN_DIR=${GROK_OPENAI_BIN_DIR:-$HOME/.local/bin}
GROK_OPENAI_LIBEXEC_DIR=${GROK_OPENAI_LIBEXEC_DIR:-$HOME/.local/libexec/grok-openai}
PROFILE_SOURCE=${GROK_OPENAI_PROFILE_SOURCE:-$REPO_ROOT/config/openai.toml}
KEYCHAIN_SERVICE=${GROK_OPENAI_KEYCHAIN_SERVICE:-ocque41.grok-build.openai}
KEYCHAIN_ACCOUNT=${GROK_OPENAI_KEYCHAIN_ACCOUNT:-openai-platform}

case "$GROK_OPENAI_HOME$GROK_OPENAI_BIN_DIR$GROK_OPENAI_LIBEXEC_DIR" in
    *'
'*) openai_workflow_die 'install paths must not contain newline characters' ;;
esac
case $HOME in
    /*) ;;
    *) openai_workflow_die 'HOME must be an absolute path' ;;
esac
case $GROK_OPENAI_HOME in
    /*) ;;
    *) openai_workflow_die 'GROK_OPENAI_HOME must be an absolute path' ;;
esac
case "$GROK_OPENAI_HOME/" in
    "$HOME/.grok/"|"$HOME/.grok/"*)
        openai_workflow_die 'refusing to use or modify the legacy ~/.grok directory'
        ;;
esac
case $GROK_OPENAI_HOME in
    */../*|*/..|*/./*|*/.) openai_workflow_die 'GROK_OPENAI_HOME must not contain . or .. path components' ;;
esac
[ ! -L "$GROK_OPENAI_HOME" ] || \
    openai_workflow_die 'GROK_OPENAI_HOME must not be a symbolic link'

[ -f "$PROFILE_SOURCE" ] || openai_workflow_die "OpenAI profile not found: $PROFILE_SOURCE"

if [ -n "${GROK_OPENAI_PREBUILT:-}" ]; then
    BINARY=$(openai_workflow_abspath "$GROK_OPENAI_PREBUILT") || \
        openai_workflow_die "cannot resolve prebuilt binary: $GROK_OPENAI_PREBUILT"
else
    CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
    case $CARGO_TARGET_DIR in
        /*) ;;
        *) CARGO_TARGET_DIR=$REPO_ROOT/$CARGO_TARGET_DIR ;;
    esac
    export CARGO_TARGET_DIR
    "$SCRIPT_DIR/validate-openai.sh"
    BINARY=$CARGO_TARGET_DIR/release-dist/xai-grok-pager
fi

[ -f "$BINARY" ] && [ -x "$BINARY" ] || openai_workflow_die "binary is not executable: $BINARY"
"$BINARY" --version >/dev/null || openai_workflow_die 'candidate binary failed its --version smoke test'
BINARY_VERSION=$("$BINARY" --version 2>&1 | sed -n '1p')

mkdir -p "$GROK_OPENAI_HOME" "$GROK_OPENAI_BIN_DIR" "$GROK_OPENAI_LIBEXEC_DIR"
chmod 700 "$GROK_OPENAI_HOME"

BINARY_DEST=$GROK_OPENAI_LIBEXEC_DIR/grok
PROFILE_DEST=$GROK_OPENAI_LIBEXEC_DIR/openai.toml
CONFIG_DEST=$GROK_OPENAI_HOME/config.toml
CONFIG_HASH_DEST=$GROK_OPENAI_HOME/.installed-config.sha256
LAUNCHER_DEST=$GROK_OPENAI_BIN_DIR/grok-openai

[ ! -L "$CONFIG_DEST" ] || \
    openai_workflow_die 'refusing a symbolic-link runtime config'

openai_workflow_atomic_copy "$BINARY" "$BINARY_DEST" 755 || \
    openai_workflow_die "failed to install binary at $BINARY_DEST"
openai_workflow_atomic_copy "$PROFILE_SOURCE" "$PROFILE_DEST" 600 || \
    openai_workflow_die "failed to install canonical profile at $PROFILE_DEST"

NEW_PROFILE_HASH=$(openai_workflow_hash_file "$PROFILE_SOURCE")
INSTALL_CONFIG=0
if [ ! -e "$CONFIG_DEST" ]; then
    INSTALL_CONFIG=1
elif [ -f "$CONFIG_HASH_DEST" ]; then
    RECORDED_HASH=$(sed -n '1p' "$CONFIG_HASH_DEST" 2>/dev/null || true)
    CURRENT_HASH=$(openai_workflow_hash_file "$CONFIG_DEST" 2>/dev/null || true)
    if [ -n "$RECORDED_HASH" ] && [ "$RECORDED_HASH" = "$CURRENT_HASH" ]; then
        INSTALL_CONFIG=1
    elif [ "$CURRENT_HASH" = "$NEW_PROFILE_HASH" ]; then
        INSTALL_CONFIG=1
    fi
fi

if [ "$INSTALL_CONFIG" -eq 1 ]; then
    openai_workflow_atomic_copy "$PROFILE_SOURCE" "$CONFIG_DEST" 600 || \
        openai_workflow_die "failed to install profile at $CONFIG_DEST"
    openai_workflow_atomic_text "$CONFIG_HASH_DEST" 600 "$NEW_PROFILE_HASH" || \
        openai_workflow_die "failed to record installed profile hash"
else
    openai_workflow_note "Preserved customized profile: $CONFIG_DEST"
    openai_workflow_note "The new canonical profile is available at: $PROFILE_DEST"
fi

DEFAULT_HOME_QUOTED=$(openai_workflow_shell_quote "$GROK_OPENAI_HOME")
DEFAULT_LIBEXEC_QUOTED=$(openai_workflow_shell_quote "$GROK_OPENAI_LIBEXEC_DIR")
DEFAULT_SERVICE_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_SERVICE")
DEFAULT_ACCOUNT_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_ACCOUNT")

LAUNCHER_TEMP=$(mktemp "${LAUNCHER_DEST}.tmp.XXXXXX") || \
    openai_workflow_die 'failed to stage launcher'
cat >"$LAUNCHER_TEMP" <<EOF
#!/bin/sh
set -eu

GROK_OPENAI_HOME=\${GROK_OPENAI_HOME:-$DEFAULT_HOME_QUOTED}
GROK_OPENAI_LIBEXEC_DIR=\${GROK_OPENAI_LIBEXEC_DIR:-$DEFAULT_LIBEXEC_QUOTED}
GROK_OPENAI_KEYCHAIN_SERVICE=\${GROK_OPENAI_KEYCHAIN_SERVICE:-$DEFAULT_SERVICE_QUOTED}
GROK_OPENAI_KEYCHAIN_ACCOUNT=\${GROK_OPENAI_KEYCHAIN_ACCOUNT:-$DEFAULT_ACCOUNT_QUOTED}

[ -n "\${HOME:-}" ] || {
    printf '%s\n' 'error: HOME is not set.' >&2
    exit 1
}
case \$GROK_OPENAI_HOME in
    /*) ;;
    *)
        printf '%s\n' 'error: GROK_OPENAI_HOME must be an absolute path.' >&2
        exit 1
        ;;
esac
case \$GROK_OPENAI_HOME in
    */../*|*/..|*/./*|*/.)
        printf '%s\n' 'error: GROK_OPENAI_HOME must not contain . or .. path components.' >&2
        exit 1
        ;;
esac
case "\$GROK_OPENAI_HOME/" in
    "\$HOME/.grok/"|"\$HOME/.grok/"*)
        printf '%s\n' 'error: refusing to use the legacy ~/.grok directory.' >&2
        exit 1
        ;;
esac
[ ! -L "\$GROK_OPENAI_HOME" ] || {
    printf '%s\n' 'error: GROK_OPENAI_HOME must not be a symbolic link.' >&2
    exit 1
}

export GROK_HOME=\$GROK_OPENAI_HOME
export GROK_DISABLE_AUTOUPDATER=1
export GROK_OPENAI_DISABLE_VENDOR_UPDATE=1
export GROK_IMAGE_GEN=0
export GROK_IMAGE_EDIT=0
export GROK_VOICE_MODE=0
export GROK_MANAGED_CONFIG=0
export GROK_DISABLE_CUSTOM_BRIDGE=1
unset XAI_API_KEY GROK_CODE_XAI_API_KEY GROK_AUTH GROK_AUTH_PATH \
    GROK_AUTH_PROVIDER_COMMAND GROK_AUTH_PROVIDER_LABEL 2>/dev/null || true

case \${1:-} in
    update)
        printf '%s\n' 'error: the vendor updater is disabled for grok-openai.' >&2
        printf '%s\n' 'Run scripts/update-from-upstream.sh from your fork checkout instead.' >&2
        exit 2
        ;;
esac

grok_openai_needs_key=1
for grok_openai_arg do
    case \$grok_openai_arg in
        -h|--help|-v|-V|--version|models) grok_openai_needs_key=0 ;;
    esac
done

if [ \$grok_openai_needs_key -eq 1 ] && [ -z "\${OPENAI_API_KEY:-}" ] && [ -x /usr/bin/security ]; then
    OPENAI_API_KEY=\$(/usr/bin/security find-generic-password \
        -a "\$GROK_OPENAI_KEYCHAIN_ACCOUNT" \
        -s "\$GROK_OPENAI_KEYCHAIN_SERVICE" \
        -w 2>/dev/null || true)
    export OPENAI_API_KEY
fi

if [ \$grok_openai_needs_key -eq 1 ] && [ -z "\${OPENAI_API_KEY:-}" ]; then
    printf '%s\n' 'error: no OpenAI Platform API key is available.' >&2
    printf '%s\n' 'Export OPENAI_API_KEY or run scripts/setup-openai-key.sh from the fork checkout.' >&2
    exit 1
fi

exec "\$GROK_OPENAI_LIBEXEC_DIR/grok" --no-auto-update "\$@"
EOF
chmod 755 "$LAUNCHER_TEMP"
/bin/sh -n "$LAUNCHER_TEMP" || {
    rm -f "$LAUNCHER_TEMP"
    openai_workflow_die 'generated launcher failed shell syntax validation'
}
mv -f "$LAUNCHER_TEMP" "$LAUNCHER_DEST" || {
    rm -f "$LAUNCHER_TEMP"
    openai_workflow_die "failed to install launcher at $LAUNCHER_DEST"
}

"$LAUNCHER_DEST" --version >/dev/null || \
    openai_workflow_die 'installed launcher failed its --version smoke test'

openai_workflow_note "Installed grok-openai: $LAUNCHER_DEST"
openai_workflow_note "Installed version: $BINARY_VERSION"
openai_workflow_note "Isolated state: $GROK_OPENAI_HOME"
case :$PATH: in
    *:"$GROK_OPENAI_BIN_DIR":*) ;;
    *) openai_workflow_note "Launch it by absolute path (the installer did not edit your shell PATH): $LAUNCHER_DEST" ;;
esac
