#!/bin/sh
set -eu

MODE=dry-run
LEGACY_ONLY=0
REMOVE_OFFICIAL_GROK=0
while [ "$#" -gt 0 ]; do
case $1 in
    --dry-run) MODE=dry-run ;;
    --apply) MODE=apply ;;
    --legacy-only) LEGACY_ONLY=1 ;;
    --remove-official-grok) REMOVE_OFFICIAL_GROK=1 ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/uninstall-bandicot.sh [--dry-run|--apply] [OPTIONS]

Preview (the default) or remove installer-owned Bandicot executables and the
legacy Bandicot grok-openai alias. User data in ~/.bandicot, ~/.grok-openai,
~/.grok-codex-plan, and ~/.grok is always preserved.

Options:
  --legacy-only          preserve the canonical Bandicot launcher and payload
  --remove-official-grok remove npm's @xai-official/grok package (apply only)
EOF
        exit 0
        ;;
    *) printf 'error: unknown argument: %s\n' "$1" >&2; exit 1 ;;
esac
shift
done

[ -n "${HOME:-}" ] || { printf '%s\n' 'error: HOME is not set' >&2; exit 1; }
BIN_DIR=${BANDICOT_BIN_DIR:-$HOME/.local/bin}
LIBEXEC_DIR=${BANDICOT_LIBEXEC_DIR:-$HOME/.local/libexec/bandicot}
LEGACY_LIBEXEC_DIR=${GROK_OPENAI_LIBEXEC_DIR:-$HOME/.local/libexec/grok-openai}
LAUNCHER=$BIN_DIR/bandicot
LEGACY_ALIAS=$BIN_DIR/grok-openai

act_remove_file() {
    if [ "$MODE" = dry-run ]; then
        printf 'Would remove: %s\n' "$1"
    else
        rm -f "$1"
        printf 'Removed: %s\n' "$1"
    fi
}

is_bandicot_launcher() {
    [ -f "$1" ] && [ ! -L "$1" ] && \
        grep -q '^# BANDICOT_INSTALLER_PROVENANCE=bandicot-local-v1$' "$1" 2>/dev/null
}

is_legacy_bandicot_alias() {
    [ -f "$1" ] && [ ! -L "$1" ] || return 1
    if grep -q '^# BANDICOT_INSTALLER_PROVENANCE=bandicot-local-v1$' "$1" 2>/dev/null; then
        return 0
    fi
    grep -q 'GROK_OPENAI_LIBEXEC_DIR' "$1" 2>/dev/null && \
        grep -q '/grok-openai/bandicot\|GROK_OPENAI_LIBEXEC_DIR/bandicot' "$1" 2>/dev/null
}

current_owned=0
if [ "$LEGACY_ONLY" -eq 0 ]; then
    if is_bandicot_launcher "$LAUNCHER"; then
        current_owned=1
        act_remove_file "$LAUNCHER"
        [ -f "$LIBEXEC_DIR/bandicot" ] && [ ! -L "$LIBEXEC_DIR/bandicot" ] && act_remove_file "$LIBEXEC_DIR/bandicot"
        [ -f "$LIBEXEC_DIR/bandicot-apple-foundation-models" ] && [ ! -L "$LIBEXEC_DIR/bandicot-apple-foundation-models" ] && act_remove_file "$LIBEXEC_DIR/bandicot-apple-foundation-models"
    elif [ -e "$LAUNCHER" ]; then
        printf 'Preserved unverified command: %s\n' "$LAUNCHER" >&2
    fi
fi

legacy_owned=0
if is_legacy_bandicot_alias "$LEGACY_ALIAS"; then
    legacy_owned=1
    act_remove_file "$LEGACY_ALIAS"
    for legacy_file in grok bandicot openai.toml codex-plan.toml; do
        [ -f "$LEGACY_LIBEXEC_DIR/$legacy_file" ] && [ ! -L "$LEGACY_LIBEXEC_DIR/$legacy_file" ] && \
            act_remove_file "$LEGACY_LIBEXEC_DIR/$legacy_file"
    done
elif [ -e "$LEGACY_ALIAS" ]; then
    printf 'Preserved unverified legacy alias: %s\n' "$LEGACY_ALIAS" >&2
fi

if command -v npm >/dev/null 2>&1 && npm list -g --depth=0 @xai-official/grok >/dev/null 2>&1; then
    if [ "$REMOVE_OFFICIAL_GROK" -eq 1 ]; then
        if [ "$MODE" = dry-run ]; then
            printf '%s\n' 'Would remove npm package: @xai-official/grok'
        else
            npm uninstall -g @xai-official/grok
            printf '%s\n' 'Removed npm package: @xai-official/grok'
        fi
    else
        printf '%s\n' 'Detected official @xai-official/grok; use --remove-official-grok to remove it.'
    fi
elif command -v grok >/dev/null 2>&1; then
    printf 'Detected a grok command at %s; it was not modified.\n' "$(command -v grok)"
else
    printf '%s\n' 'Official @xai-official/grok was not detected.'
fi

printf '%s\n' 'Preserved all profile and session data, including ~/.grok.'
if [ "$current_owned" -eq 0 ] && [ "$legacy_owned" -eq 0 ]; then
    printf '%s\n' 'No installer-owned commands were found.'
fi
