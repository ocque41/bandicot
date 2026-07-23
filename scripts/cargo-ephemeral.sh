#!/bin/sh
set -eu

EPHEMERAL_TARGET=$(mktemp -d "${TMPDIR:-/tmp}/bandicot-cargo.XXXXXX")
cleanup() {
    rm -rf "$EPHEMERAL_TARGET"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

export CARGO_TARGET_DIR="$EPHEMERAL_TARGET"
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0

cargo "$@"
