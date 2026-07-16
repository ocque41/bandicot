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
  GROK_CODEX_PLAN_HOME        Codex-plan state dir (default: ~/.grok-codex-plan)
  GROK_OPENAI_BIN_DIR         launcher dir (default: ~/.local/bin)
  GROK_OPENAI_LIBEXEC_DIR     binary dir (default: ~/.local/libexec/grok-openai)
  GROK_CODEX_PROXY_TOKEN_FILE CLIProxyAPI client token (default: ~/.cli-proxy-api/client-token)
EOF
        exit 0
        ;;
    *) openai_workflow_die "unknown argument: $1" ;;
esac

[ -n "${HOME:-}" ] || openai_workflow_die 'HOME is not set'

GROK_OPENAI_HOME=${GROK_OPENAI_HOME:-$HOME/.grok-openai}
GROK_CODEX_PLAN_HOME=${GROK_CODEX_PLAN_HOME:-$HOME/.grok-codex-plan}
GROK_OPENAI_BIN_DIR=${GROK_OPENAI_BIN_DIR:-$HOME/.local/bin}
GROK_OPENAI_LIBEXEC_DIR=${GROK_OPENAI_LIBEXEC_DIR:-$HOME/.local/libexec/grok-openai}
PROFILE_SOURCE=${GROK_OPENAI_PROFILE_SOURCE:-$REPO_ROOT/config/openai.toml}
PLAN_PROFILE_SOURCE=${GROK_CODEX_PLAN_PROFILE_SOURCE:-$REPO_ROOT/config/codex-plan.toml}
PROXY_TOKEN_FILE=${GROK_CODEX_PROXY_TOKEN_FILE:-$HOME/.cli-proxy-api/client-token}
KEYCHAIN_SERVICE=${GROK_OPENAI_KEYCHAIN_SERVICE:-ocque41.grok-build.openai}
KEYCHAIN_ACCOUNT=${GROK_OPENAI_KEYCHAIN_ACCOUNT:-openai-platform}

case "$GROK_OPENAI_HOME$GROK_CODEX_PLAN_HOME$GROK_OPENAI_BIN_DIR$GROK_OPENAI_LIBEXEC_DIR$PROXY_TOKEN_FILE" in
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
case $GROK_CODEX_PLAN_HOME in
    /*) ;;
    *) openai_workflow_die 'GROK_CODEX_PLAN_HOME must be an absolute path' ;;
esac
case $PROXY_TOKEN_FILE in
    /*) ;;
    *) openai_workflow_die 'GROK_CODEX_PROXY_TOKEN_FILE must be an absolute path' ;;
esac
case "$GROK_OPENAI_HOME/" in
    "$HOME/.grok/"|"$HOME/.grok/"*)
        openai_workflow_die 'refusing to use or modify the legacy ~/.grok directory'
        ;;
esac
case "$GROK_CODEX_PLAN_HOME/" in
    "$HOME/.grok/"|"$HOME/.grok/"*)
        openai_workflow_die 'refusing to use or modify the legacy ~/.grok directory'
        ;;
esac
case $GROK_OPENAI_HOME in
    */../*|*/..|*/./*|*/.) openai_workflow_die 'GROK_OPENAI_HOME must not contain . or .. path components' ;;
esac
case $GROK_CODEX_PLAN_HOME in
    */../*|*/..|*/./*|*/.) openai_workflow_die 'GROK_CODEX_PLAN_HOME must not contain . or .. path components' ;;
esac
[ ! -L "$GROK_OPENAI_HOME" ] || \
    openai_workflow_die 'GROK_OPENAI_HOME must not be a symbolic link'
[ ! -L "$GROK_CODEX_PLAN_HOME" ] || \
    openai_workflow_die 'GROK_CODEX_PLAN_HOME must not be a symbolic link'

[ -f "$PROFILE_SOURCE" ] || openai_workflow_die "OpenAI profile not found: $PROFILE_SOURCE"
[ -f "$PLAN_PROFILE_SOURCE" ] || openai_workflow_die "Codex-plan profile not found: $PLAN_PROFILE_SOURCE"

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

mkdir -p "$GROK_OPENAI_HOME" "$GROK_CODEX_PLAN_HOME" "$GROK_OPENAI_BIN_DIR" "$GROK_OPENAI_LIBEXEC_DIR"
chmod 700 "$GROK_OPENAI_HOME" "$GROK_CODEX_PLAN_HOME"

BINARY_DEST=$GROK_OPENAI_LIBEXEC_DIR/grok
PROFILE_DEST=$GROK_OPENAI_LIBEXEC_DIR/openai.toml
PLAN_PROFILE_DEST=$GROK_OPENAI_LIBEXEC_DIR/codex-plan.toml
CONFIG_DEST=$GROK_OPENAI_HOME/config.toml
CONFIG_HASH_DEST=$GROK_OPENAI_HOME/.installed-config.sha256
PLAN_CONFIG_DEST=$GROK_CODEX_PLAN_HOME/config.toml
PLAN_CONFIG_HASH_DEST=$GROK_CODEX_PLAN_HOME/.installed-config.sha256
LAUNCHER_DEST=$GROK_OPENAI_BIN_DIR/grok-openai

[ ! -L "$CONFIG_DEST" ] || \
    openai_workflow_die 'refusing a symbolic-link runtime config'
[ ! -L "$PLAN_CONFIG_DEST" ] || \
    openai_workflow_die 'refusing a symbolic-link Codex-plan runtime config'

openai_workflow_atomic_copy "$BINARY" "$BINARY_DEST" 755 || \
    openai_workflow_die "failed to install binary at $BINARY_DEST"
openai_workflow_atomic_copy "$PROFILE_SOURCE" "$PROFILE_DEST" 600 || \
    openai_workflow_die "failed to install canonical profile at $PROFILE_DEST"
openai_workflow_atomic_copy "$PLAN_PROFILE_SOURCE" "$PLAN_PROFILE_DEST" 600 || \
    openai_workflow_die "failed to install canonical Codex-plan profile at $PLAN_PROFILE_DEST"

install_runtime_profile() {
    _irp_source=$1
    _irp_config=$2
    _irp_hash_file=$3
    _irp_canonical=$4
    _irp_new_hash=$(openai_workflow_hash_file "$_irp_source")
    _irp_install=0
    if [ ! -e "$_irp_config" ]; then
        _irp_install=1
    elif [ -f "$_irp_hash_file" ]; then
        _irp_recorded=$(sed -n '1p' "$_irp_hash_file" 2>/dev/null || true)
        _irp_current=$(openai_workflow_hash_file "$_irp_config" 2>/dev/null || true)
        if [ -n "$_irp_recorded" ] && [ "$_irp_recorded" = "$_irp_current" ]; then
            _irp_install=1
        elif [ "$_irp_current" = "$_irp_new_hash" ]; then
            _irp_install=1
        fi
    fi

    if [ "$_irp_install" -eq 1 ]; then
        openai_workflow_atomic_copy "$_irp_source" "$_irp_config" 600 || \
            openai_workflow_die "failed to install profile at $_irp_config"
        openai_workflow_atomic_text "$_irp_hash_file" 600 "$_irp_new_hash" || \
            openai_workflow_die "failed to record installed profile hash"
    else
        openai_workflow_note "Preserved customized profile: $_irp_config"
        openai_workflow_note "The new canonical profile is available at: $_irp_canonical"
    fi
}

install_runtime_profile "$PROFILE_SOURCE" "$CONFIG_DEST" "$CONFIG_HASH_DEST" "$PROFILE_DEST"
install_runtime_profile "$PLAN_PROFILE_SOURCE" "$PLAN_CONFIG_DEST" "$PLAN_CONFIG_HASH_DEST" "$PLAN_PROFILE_DEST"

DEFAULT_HOME_QUOTED=$(openai_workflow_shell_quote "$GROK_OPENAI_HOME")
DEFAULT_PLAN_HOME_QUOTED=$(openai_workflow_shell_quote "$GROK_CODEX_PLAN_HOME")
DEFAULT_LIBEXEC_QUOTED=$(openai_workflow_shell_quote "$GROK_OPENAI_LIBEXEC_DIR")
DEFAULT_SERVICE_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_SERVICE")
DEFAULT_ACCOUNT_QUOTED=$(openai_workflow_shell_quote "$KEYCHAIN_ACCOUNT")
DEFAULT_PROXY_TOKEN_FILE_QUOTED=$(openai_workflow_shell_quote "$PROXY_TOKEN_FILE")

LAUNCHER_TEMP=$(mktemp "${LAUNCHER_DEST}.tmp.XXXXXX") || \
    openai_workflow_die 'failed to stage launcher'
cat >"$LAUNCHER_TEMP" <<EOF
#!/bin/sh
set -eu

GROK_OPENAI_HOME=\${GROK_OPENAI_HOME:-$DEFAULT_HOME_QUOTED}
GROK_CODEX_PLAN_HOME=\${GROK_CODEX_PLAN_HOME:-$DEFAULT_PLAN_HOME_QUOTED}
GROK_OPENAI_LIBEXEC_DIR=\${GROK_OPENAI_LIBEXEC_DIR:-$DEFAULT_LIBEXEC_QUOTED}
GROK_OPENAI_KEYCHAIN_SERVICE=\${GROK_OPENAI_KEYCHAIN_SERVICE:-$DEFAULT_SERVICE_QUOTED}
GROK_OPENAI_KEYCHAIN_ACCOUNT=\${GROK_OPENAI_KEYCHAIN_ACCOUNT:-$DEFAULT_ACCOUNT_QUOTED}
GROK_CODEX_PROXY_TOKEN_FILE=\${GROK_CODEX_PROXY_TOKEN_FILE:-$DEFAULT_PROXY_TOKEN_FILE_QUOTED}

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
case \$GROK_CODEX_PLAN_HOME in
    /*) ;;
    *)
        printf '%s\n' 'error: GROK_CODEX_PLAN_HOME must be an absolute path.' >&2
        exit 1
        ;;
esac
case \$GROK_CODEX_PROXY_TOKEN_FILE in
    /*) ;;
    *)
        printf '%s\n' 'error: GROK_CODEX_PROXY_TOKEN_FILE must be an absolute path.' >&2
        exit 1
        ;;
esac
case \$GROK_OPENAI_HOME in
    */../*|*/..|*/./*|*/.)
        printf '%s\n' 'error: GROK_OPENAI_HOME must not contain . or .. path components.' >&2
        exit 1
        ;;
esac
case \$GROK_CODEX_PLAN_HOME in
    */../*|*/..|*/./*|*/.)
        printf '%s\n' 'error: GROK_CODEX_PLAN_HOME must not contain . or .. path components.' >&2
        exit 1
        ;;
esac
case "\$GROK_OPENAI_HOME/" in
    "\$HOME/.grok/"|"\$HOME/.grok/"*)
        printf '%s\n' 'error: refusing to use the legacy ~/.grok directory.' >&2
        exit 1
        ;;
esac
case "\$GROK_CODEX_PLAN_HOME/" in
    "\$HOME/.grok/"|"\$HOME/.grok/"*)
        printf '%s\n' 'error: refusing to use the legacy ~/.grok directory.' >&2
        exit 1
        ;;
esac
[ ! -L "\$GROK_OPENAI_HOME" ] || {
    printf '%s\n' 'error: GROK_OPENAI_HOME must not be a symbolic link.' >&2
    exit 1
}
[ ! -L "\$GROK_CODEX_PLAN_HOME" ] || {
    printf '%s\n' 'error: GROK_CODEX_PLAN_HOME must not be a symbolic link.' >&2
    exit 1
}

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
grok_openai_prefer_plan=0
case \${1:-} in
    -h|--help|-v|-V|--version|leader) grok_openai_needs_key=0 ;;
    models)
        grok_openai_needs_key=0
        grok_openai_prefer_plan=1
        ;;
esac

if [ \$grok_openai_needs_key -eq 1 ] && [ -z "\${OPENAI_API_KEY:-}" ] && [ -x /usr/bin/security ]; then
    OPENAI_API_KEY=\$(/usr/bin/security find-generic-password \
        -a "\$GROK_OPENAI_KEYCHAIN_ACCOUNT" \
        -s "\$GROK_OPENAI_KEYCHAIN_SERVICE" \
        -w 2>/dev/null || true)
    export OPENAI_API_KEY
fi

if [ -n "\${OPENAI_API_KEY:-}" ]; then
    export GROK_HOME=\$GROK_OPENAI_HOME
    exec "\$GROK_OPENAI_LIBEXEC_DIR/grok" --no-auto-update "\$@"
fi

if [ \$grok_openai_needs_key -eq 0 ] && [ \$grok_openai_prefer_plan -eq 0 ]; then
    export GROK_HOME=\$GROK_OPENAI_HOME
    exec "\$GROK_OPENAI_LIBEXEC_DIR/grok" --no-auto-update "\$@"
fi

if [ -f "\$GROK_CODEX_PROXY_TOKEN_FILE" ] && [ ! -L "\$GROK_CODEX_PROXY_TOKEN_FILE" ]; then
    grok_proxy_token_mode=\$(stat -f '%Lp' "\$GROK_CODEX_PROXY_TOKEN_FILE" 2>/dev/null || \
        stat -c '%a' "\$GROK_CODEX_PROXY_TOKEN_FILE" 2>/dev/null || true)
    case \$grok_proxy_token_mode in
        400|600) ;;
        *)
            printf '%s\n' 'error: CLIProxyAPI client token must have mode 0400 or 0600.' >&2
            exit 1
            ;;
    esac
    IFS= read -r GROK_CODEX_PROXY_TOKEN <"\$GROK_CODEX_PROXY_TOKEN_FILE" || true
    if [ -n "\$GROK_CODEX_PROXY_TOKEN" ]; then
        export GROK_CODEX_PROXY_TOKEN
        export GROK_HOME=\$GROK_CODEX_PLAN_HOME
        printf '%s\n' 'bandicot: starting Grok Build through local CLIProxyAPI with Codex/ChatGPT authentication.' >&2
        exec "\$GROK_OPENAI_LIBEXEC_DIR/grok" --no-auto-update "\$@"
    fi
fi

if [ \$grok_openai_needs_key -eq 0 ] && [ \$grok_openai_prefer_plan -eq 1 ]; then
    export GROK_HOME=\$GROK_CODEX_PLAN_HOME
    exec "\$GROK_OPENAI_LIBEXEC_DIR/grok" --no-auto-update "\$@"
fi

printf '%s\n' 'error: no Platform API key or protected CLIProxyAPI client token is available.' >&2
printf '%s\n' 'Run CLIProxyAPI Codex login, or scripts/setup-openai-key.sh for Platform access.' >&2
exit 1
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
openai_workflow_note "Codex-plan state: $GROK_CODEX_PLAN_HOME"
case :$PATH: in
    *:"$GROK_OPENAI_BIN_DIR":*) ;;
    *) openai_workflow_note "Launch it by absolute path (the installer did not edit your shell PATH): $LAUNCHER_DEST" ;;
esac
