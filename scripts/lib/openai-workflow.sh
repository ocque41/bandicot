#!/bin/sh

# Shared, POSIX-shell helpers for the OpenAI fork's local workflows.
# This file is sourced by scripts in the parent directory; it does not mutate
# shell startup files or any terminal application configuration.

openai_workflow_die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

openai_workflow_note() {
    printf '%s\n' "$*"
}

openai_workflow_require_command() {
    command -v "$1" >/dev/null 2>&1 || openai_workflow_die "required command not found: $1"
}

openai_workflow_abspath() {
    _ow_path=$1
    _ow_dir=$(dirname -- "$_ow_path")
    _ow_base=$(basename -- "$_ow_path")
    (CDPATH='' cd -- "$_ow_dir" 2>/dev/null && printf '%s/%s\n' "$(pwd -P)" "$_ow_base")
}

openai_workflow_hash_file() {
    _ow_hash_path=$1
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$_ow_hash_path" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$_ow_hash_path" | awk '{print $1}'
    else
        # cksum is not cryptographic, but is sufficient for detecting whether
        # the user edited an installed, non-secret TOML profile.
        cksum "$_ow_hash_path" | awk '{print $1 ":" $2}'
    fi
}

openai_workflow_atomic_copy() {
    _ow_source=$1
    _ow_destination=$2
    _ow_mode=$3
    _ow_destination_dir=$(dirname -- "$_ow_destination")
    mkdir -p "$_ow_destination_dir"
    _ow_temp=$(mktemp "${_ow_destination}.tmp.XXXXXX") || return 1
    if ! cp "$_ow_source" "$_ow_temp" || ! chmod "$_ow_mode" "$_ow_temp"; then
        rm -f "$_ow_temp"
        return 1
    fi
    if ! mv -f "$_ow_temp" "$_ow_destination"; then
        rm -f "$_ow_temp"
        return 1
    fi
}

openai_workflow_atomic_text() {
    _ow_destination=$1
    _ow_mode=$2
    _ow_text=$3
    _ow_destination_dir=$(dirname -- "$_ow_destination")
    mkdir -p "$_ow_destination_dir"
    _ow_temp=$(mktemp "${_ow_destination}.tmp.XXXXXX") || return 1
    if ! printf '%s\n' "$_ow_text" >"$_ow_temp" || ! chmod "$_ow_mode" "$_ow_temp"; then
        rm -f "$_ow_temp"
        return 1
    fi
    if ! mv -f "$_ow_temp" "$_ow_destination"; then
        rm -f "$_ow_temp"
        return 1
    fi
}

openai_workflow_shell_quote() {
    # Emit one shell word. Newlines in install paths are deliberately rejected
    # by install-bandicot.sh before this helper is called.
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}
