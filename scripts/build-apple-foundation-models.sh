#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)
PACKAGE_DIR=$REPO_ROOT/native/apple-foundation-models
RUN_TESTS=0
if [ "${1:-}" = --test ]; then
    RUN_TESTS=1
    shift
fi
DESTINATION=${1:-$REPO_ROOT/target/apple-foundation-models/bandicot-apple-foundation-models}

[ "$(uname -s)" = Darwin ] || { printf '%s\n' 'error: Apple Foundation Models helper can only be built on macOS' >&2; exit 1; }
command -v swift >/dev/null 2>&1 || { printf '%s\n' 'error: Swift is required to build the Apple Foundation Models helper' >&2; exit 1; }

if [ "$RUN_TESTS" -eq 1 ]; then
    swift test --package-path "$PACKAGE_DIR"
fi
swift build --package-path "$PACKAGE_DIR" -c release
SOURCE=$(swift build --package-path "$PACKAGE_DIR" -c release --show-bin-path)/bandicot-apple-foundation-models
mkdir -p "$(dirname -- "$DESTINATION")"
TEMP=$(mktemp "${DESTINATION}.tmp.XXXXXX")
trap 'rm -f "$TEMP"' 0 HUP INT TERM
cp "$SOURCE" "$TEMP"
chmod 755 "$TEMP"
mv -f "$TEMP" "$DESTINATION"
trap - 0 HUP INT TERM
printf '%s\n' "$DESTINATION"
