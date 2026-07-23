#!/bin/sh
set -eu
umask 022

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)

usage() {
    cat <<'EOF'
Usage: scripts/package-macos-release.sh VERSION BINARY APPLE_HELPER OUTPUT_DIR

Create the prebuilt Apple Silicon macOS release archive used by GitHub Releases
and Homebrew. VERSION must not include the leading "v".
EOF
}

[ "$#" -eq 4 ] || { usage >&2; exit 2; }
VERSION=$1
BINARY=$2
APPLE_HELPER=$3
OUTPUT_DIR=$4

case $VERSION in
    ''|*[!0-9A-Za-z.+-]*) printf 'error: invalid version: %s\n' "$VERSION" >&2; exit 1 ;;
esac

abspath() {
    _path=$1
    _dir=$(dirname -- "$_path")
    _base=$(basename -- "$_path")
    (CDPATH='' cd -- "$_dir" && printf '%s/%s\n' "$PWD" "$_base")
}

BINARY=$(abspath "$BINARY")
APPLE_HELPER=$(abspath "$APPLE_HELPER")
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR=$(abspath "$OUTPUT_DIR")

[ -x "$BINARY" ] || { printf 'error: binary is not executable: %s\n' "$BINARY" >&2; exit 1; }
[ -x "$APPLE_HELPER" ] || { printf 'error: Apple helper is not executable: %s\n' "$APPLE_HELPER" >&2; exit 1; }

if [ "${BANDICOT_PACKAGE_TEST_MODE:-0}" != 1 ]; then
    command -v file >/dev/null 2>&1 || { printf '%s\n' 'error: file is required' >&2; exit 1; }
    file "$BINARY" | grep -q 'Mach-O 64-bit executable arm64' || {
        printf '%s\n' 'error: Bandicot release binary is not an Apple Silicon Mach-O executable' >&2
        exit 1
    }
    file "$APPLE_HELPER" | grep -q 'Mach-O 64-bit executable arm64' || {
        printf '%s\n' 'error: Apple helper is not an Apple Silicon Mach-O executable' >&2
        exit 1
    }
fi

WORK_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/bandicot-package.XXXXXX")
cleanup() {
    rm -rf "$WORK_ROOT"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

ARCHIVE_ROOT=bandicot-$VERSION-aarch64-apple-darwin
STAGE=$WORK_ROOT/$ARCHIVE_ROOT
TEST_HOME=$WORK_ROOT/home
mkdir -p "$STAGE/bin" "$STAGE/libexec/bandicot" "$STAGE/share/bandicot" "$TEST_HOME"

HOME=$TEST_HOME \
BANDICOT_HOME=$TEST_HOME/.bandicot \
BANDICOT_BIN_DIR=$STAGE/bin \
BANDICOT_LIBEXEC_DIR=$STAGE/libexec/bandicot \
BANDICOT_PREBUILT=$BINARY \
BANDICOT_APPLE_HELPER_PREBUILT=$APPLE_HELPER \
BANDICOT_PROFILE_SOURCE=$REPO_ROOT/config/openai.toml \
    "$REPO_ROOT/scripts/install-bandicot.sh" >/dev/null

LAUNCHER=$STAGE/bin/bandicot
PORTABLE_LAUNCHER=$WORK_ROOT/bandicot-launcher
awk '
    /^BANDICOT_HOME=/ {
        print "BANDICOT_HOME=${BANDICOT_HOME:-${HOME:-}/.bandicot}"
        next
    }
    /^BANDICOT_LIBEXEC_DIR=/ {
        print "BANDICOT_LIBEXEC_DIR=${BANDICOT_LIBEXEC_DIR:-'\''@@BANDICOT_LIBEXEC_DIR@@'\''}"
        next
    }
    /^BANDICOT_PROXY_TOKEN_FILE=/ {
        print "BANDICOT_PROXY_TOKEN_FILE=${BANDICOT_PROXY_TOKEN_FILE:-${HOME:-}/.cli-proxy-api/client-token}"
        next
    }
    { print }
' "$LAUNCHER" >"$PORTABLE_LAUNCHER"
chmod 755 "$PORTABLE_LAUNCHER"
/bin/sh -n "$PORTABLE_LAUNCHER"
mv "$PORTABLE_LAUNCHER" "$LAUNCHER"

if grep -F "$WORK_ROOT" "$LAUNCHER" >/dev/null 2>&1; then
    printf '%s\n' 'error: release launcher contains a temporary packaging path' >&2
    exit 1
fi
grep -F '@@BANDICOT_LIBEXEC_DIR@@' "$LAUNCHER" >/dev/null || {
    printf '%s\n' 'error: release launcher is missing the Homebrew libexec token' >&2
    exit 1
}

cp "$REPO_ROOT/config/openai.toml" "$STAGE/share/bandicot/openai.toml"
cp "$REPO_ROOT/LICENSE" "$STAGE/share/bandicot/LICENSE"
cp "$REPO_ROOT/THIRD-PARTY-NOTICES" "$STAGE/share/bandicot/THIRD-PARTY-NOTICES"
cp "$REPO_ROOT/FORK-NOTICE.md" "$STAGE/share/bandicot/FORK-NOTICE.md"

ARCHIVE=$OUTPUT_DIR/$ARCHIVE_ROOT.tar.gz
(
    cd "$WORK_ROOT"
    COPYFILE_DISABLE=1 tar -czf "$ARCHIVE" "$ARCHIVE_ROOT"
)

ARCHIVE_NAME=$(basename -- "$ARCHIVE")
if command -v shasum >/dev/null 2>&1; then
    (cd "$OUTPUT_DIR" && shasum -a 256 "$ARCHIVE_NAME" >"$ARCHIVE_NAME.sha256")
else
    (cd "$OUTPUT_DIR" && sha256sum "$ARCHIVE_NAME" >"$ARCHIVE_NAME.sha256")
fi

printf '%s\n' "$ARCHIVE"
