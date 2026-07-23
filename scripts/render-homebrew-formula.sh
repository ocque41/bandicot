#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)

[ "$#" -eq 3 ] || {
    printf '%s\n' 'Usage: scripts/render-homebrew-formula.sh VERSION ARCHIVE OUTPUT' >&2
    exit 2
}

VERSION=$1
ARCHIVE=$2
OUTPUT=$3
TEMPLATE=$REPO_ROOT/packaging/homebrew/Formula/bandicot.rb.tmpl

[ -f "$ARCHIVE" ] || { printf 'error: archive not found: %s\n' "$ARCHIVE" >&2; exit 1; }
[ -f "$TEMPLATE" ] || { printf 'error: formula template not found: %s\n' "$TEMPLATE" >&2; exit 1; }

case $VERSION in
    ''|*[!0-9A-Za-z.+-]*) printf 'error: invalid version: %s\n' "$VERSION" >&2; exit 1 ;;
esac

if command -v shasum >/dev/null 2>&1; then
    SHA256=$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')
else
    SHA256=$(sha256sum "$ARCHIVE" | awk '{print $1}')
fi

mkdir -p "$(dirname -- "$OUTPUT")"
sed \
    -e "s/@@VERSION@@/$VERSION/g" \
    -e "s/@@SHA256@@/$SHA256/g" \
    "$TEMPLATE" >"$OUTPUT"

command -v ruby >/dev/null 2>&1 && ruby -c "$OUTPUT" >/dev/null
printf '%s\n' "$OUTPUT"
