#!/bin/sh
set -eu

BANDICOT_HOME=${BANDICOT_HOME:-$HOME/.bandicot}
mkdir -p "$BANDICOT_HOME"
chmod 700 "$BANDICOT_HOME"

printf 'Enter your OpenCode Go API key: '
IFS= read -r OPENCODE_GO_KEY || true
case "$OPENCODE_GO_KEY" in '') printf '%s\n' 'No key provided.' >&2; exit 1 ;; esac

printf '%s' "$OPENCODE_GO_KEY" > "$BANDICOT_HOME/opencode-go.key"
chmod 600 "$BANDICOT_HOME/opencode-go.key"
printf '%s\n' "OpenCode Go key stored in $BANDICOT_HOME/opencode-go.key"
