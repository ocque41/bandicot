#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)

if [ -n "${GROK_OPENAI_PREBUILT:-}" ] && [ -z "${BANDICOT_PREBUILT:-}" ]; then BANDICOT_PREBUILT=$GROK_OPENAI_PREBUILT; export BANDICOT_PREBUILT; fi
if [ -n "${GROK_OPENAI_PROFILE_SOURCE:-}" ] && [ -z "${BANDICOT_PROFILE_SOURCE:-}" ]; then BANDICOT_PROFILE_SOURCE=$GROK_OPENAI_PROFILE_SOURCE; export BANDICOT_PROFILE_SOURCE; fi
if [ -n "${GROK_OPENAI_HOME:-}" ] && [ -z "${BANDICOT_HOME:-}" ]; then BANDICOT_HOME=$GROK_OPENAI_HOME; export BANDICOT_HOME; fi
if [ -n "${GROK_OPENAI_BIN_DIR:-}" ] && [ -z "${BANDICOT_BIN_DIR:-}" ]; then BANDICOT_BIN_DIR=$GROK_OPENAI_BIN_DIR; export BANDICOT_BIN_DIR; fi
if [ -n "${GROK_OPENAI_LIBEXEC_DIR:-}" ] && [ -z "${BANDICOT_LIBEXEC_DIR:-}" ]; then BANDICOT_LIBEXEC_DIR=$GROK_OPENAI_LIBEXEC_DIR; export BANDICOT_LIBEXEC_DIR; fi
if [ -n "${GROK_CODEX_PROXY_TOKEN_FILE:-}" ] && [ -z "${BANDICOT_PROXY_TOKEN_FILE:-}" ]; then BANDICOT_PROXY_TOKEN_FILE=$GROK_CODEX_PROXY_TOKEN_FILE; export BANDICOT_PROXY_TOKEN_FILE; fi

printf '%s\n' 'install-openai.sh is deprecated; use scripts/install-bandicot.sh.' >&2
exec "$SCRIPT_DIR/install-bandicot.sh" "$@"
