#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd -P)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/bandicot-release-tests.XXXXXX")
cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup 0

make_fake_binary() {
    cat >"$1" <<'EOF'
#!/bin/sh
case ${1:-} in
    -v|-V|--version) printf '%s\n' 'bandicot 0.2.110'; exit 0 ;;
esac
exit 0
EOF
    chmod 755 "$1"
}

make_fake_binary "$TEST_ROOT/bandicot"
make_fake_binary "$TEST_ROOT/apple-helper"
mkdir -p "$TEST_ROOT/dist"

ARCHIVE=$(BANDICOT_PACKAGE_TEST_MODE=1 \
    "$REPO_ROOT/scripts/package-macos-release.sh" \
        0.2.110 \
        "$TEST_ROOT/bandicot" \
        "$TEST_ROOT/apple-helper" \
        "$TEST_ROOT/dist")

[ -f "$ARCHIVE" ] || { printf '%s\n' 'FAIL: release archive missing' >&2; exit 1; }
[ -f "$ARCHIVE.sha256" ] || { printf '%s\n' 'FAIL: release checksum missing' >&2; exit 1; }
(
    cd "$(dirname -- "$ARCHIVE")"
    shasum -a 256 -c "$(basename -- "$ARCHIVE").sha256" >/dev/null
)

mkdir -p "$TEST_ROOT/extracted"
tar -xzf "$ARCHIVE" -C "$TEST_ROOT/extracted"
ROOT=$TEST_ROOT/extracted/bandicot-0.2.110-aarch64-apple-darwin
LAUNCHER=$ROOT/bin/bandicot

grep -F '@@BANDICOT_LIBEXEC_DIR@@' "$LAUNCHER" >/dev/null || {
    printf '%s\n' 'FAIL: portable launcher token missing' >&2
    exit 1
}
if grep -F "$TEST_ROOT" "$LAUNCHER" >/dev/null 2>&1; then
    printf '%s\n' 'FAIL: portable launcher leaked a packaging path' >&2
    exit 1
fi
[ -x "$ROOT/libexec/bandicot/bandicot" ] || { printf '%s\n' 'FAIL: payload missing' >&2; exit 1; }
[ -x "$ROOT/libexec/bandicot/bandicot-apple-foundation-models" ] || {
    printf '%s\n' 'FAIL: Apple helper missing' >&2
    exit 1
}
[ -f "$ROOT/share/bandicot/openai.toml" ] || { printf '%s\n' 'FAIL: profile missing' >&2; exit 1; }

PORTABLE_LAUNCHER=$TEST_ROOT/installed-bandicot
awk -v libexec="$ROOT/libexec/bandicot" \
    '{ gsub(/@@BANDICOT_LIBEXEC_DIR@@/, libexec); print }' \
    "$LAUNCHER" >"$PORTABLE_LAUNCHER"
chmod 755 "$PORTABLE_LAUNCHER"
HOME=$TEST_ROOT/runtime-home "$PORTABLE_LAUNCHER" --version >/dev/null

FORMULA=$TEST_ROOT/Formula/bandicot.rb
"$REPO_ROOT/scripts/render-homebrew-formula.sh" 0.2.110 "$ARCHIVE" "$FORMULA" >/dev/null
grep -F 'version "0.2.110"' "$FORMULA" >/dev/null || {
    printf '%s\n' 'FAIL: formula version missing' >&2
    exit 1
}
grep -F '@@VERSION@@' "$FORMULA" >/dev/null 2>&1 && {
    printf '%s\n' 'FAIL: formula version token remained' >&2
    exit 1
}

NOTES=$TEST_ROOT/release-notes.md
"$REPO_ROOT/scripts/extract-release-notes.sh" 0.2.111 "$REPO_ROOT/CHANGELOG.md" >"$NOTES"
grep -F '## [0.2.111]' "$NOTES" >/dev/null || {
    printf '%s\n' 'FAIL: release notes heading missing' >&2
    exit 1
}
if grep -F '## [0.2.110]' "$NOTES" >/dev/null 2>&1; then
    printf '%s\n' 'FAIL: release notes included the next version' >&2
    exit 1
fi
if "$REPO_ROOT/scripts/extract-release-notes.sh" 9.9.9 "$REPO_ROOT/CHANGELOG.md" >"$NOTES"; then
    printf '%s\n' 'FAIL: missing release notes unexpectedly succeeded' >&2
    exit 1
fi

printf '%s\n' 'Release packaging tests passed.'
