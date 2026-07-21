#!/bin/sh
set -eu

BANDICOT_HOME=${BANDICOT_HOME:-$HOME/.bandicot}
mkdir -p "$BANDICOT_HOME"
chmod 700 "$BANDICOT_HOME"

printf 'Enter your OpenAI API key: '
IFS= read -r OPENAI_KEY || true
case "$OPENAI_KEY" in '') printf '%s\n' 'No key provided.' >&2; exit 1 ;; esac

printf '%s' "$OPENAI_KEY" > "$BANDICOT_HOME/openai.key"
chmod 600 "$BANDICOT_HOME/openai.key"
printf '%s\n' "OpenAI key stored in $BANDICOT_HOME/openai.key"
