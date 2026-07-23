#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)
WORKFLOW=macos-arm64.yml
REPOSITORY=ocque41/bandicot

command -v gh >/dev/null 2>&1 || {
    printf '%s\n' 'error: GitHub CLI is required: https://cli.github.com/' >&2
    exit 1
}
gh auth status >/dev/null 2>&1 || {
    printf '%s\n' 'error: GitHub CLI is not authenticated. Run: gh auth login -h github.com' >&2
    exit 1
}

cd "$REPO_ROOT"
[ -z "$(git status --porcelain)" ] || {
    printf '%s\n' 'error: commit your changes before requesting a remote build' >&2
    exit 1
}

BRANCH=$(git branch --show-current)
[ -n "$BRANCH" ] || { printf '%s\n' 'error: detached HEAD cannot request a remote build' >&2; exit 1; }
LOCAL_SHA=$(git rev-parse HEAD)
REMOTE_SHA=$(git ls-remote origin "refs/heads/$BRANCH" | awk 'NR == 1 {print $1}')
[ "$LOCAL_SHA" = "$REMOTE_SHA" ] || {
    printf 'error: push branch %s before requesting a remote build\n' "$BRANCH" >&2
    exit 1
}

STARTED_AT=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
gh workflow run "$WORKFLOW" --repo "$REPOSITORY" --ref "$BRANCH"

RUN_ID=
_attempt=0
while [ -z "$RUN_ID" ] && [ "$_attempt" -lt 20 ]; do
    _attempt=$((_attempt + 1))
    RUN_ID=$(gh run list \
        --repo "$REPOSITORY" \
        --workflow "$WORKFLOW" \
        --branch "$BRANCH" \
        --event workflow_dispatch \
        --created ">=$STARTED_AT" \
        --limit 1 \
        --json databaseId \
        --jq '.[0].databaseId // empty')
    [ -n "$RUN_ID" ] || sleep 3
done
[ -n "$RUN_ID" ] || { printf '%s\n' 'error: could not locate the queued GitHub build' >&2; exit 1; }

gh run watch "$RUN_ID" --repo "$REPOSITORY" --exit-status

DOWNLOAD_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/bandicot-remote-build.XXXXXX")
cleanup() {
    rm -rf "$DOWNLOAD_ROOT"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

gh run download "$RUN_ID" \
    --repo "$REPOSITORY" \
    --name bandicot-macos-arm64 \
    --dir "$DOWNLOAD_ROOT"

ARCHIVE=$(find "$DOWNLOAD_ROOT" -type f -name 'bandicot-*-aarch64-apple-darwin.tar.gz' -print | head -1)
[ -n "$ARCHIVE" ] || { printf '%s\n' 'error: remote build archive was not downloaded' >&2; exit 1; }
tar -xzf "$ARCHIVE" -C "$DOWNLOAD_ROOT"
PACKAGE_ROOT=$(find "$DOWNLOAD_ROOT" -mindepth 1 -maxdepth 1 -type d -name 'bandicot-*-aarch64-apple-darwin' -print | head -1)
[ -n "$PACKAGE_ROOT" ] || { printf '%s\n' 'error: remote build archive layout is invalid' >&2; exit 1; }

BANDICOT_PREBUILT=$PACKAGE_ROOT/libexec/bandicot/bandicot \
BANDICOT_APPLE_HELPER_PREBUILT=$PACKAGE_ROOT/libexec/bandicot/bandicot-apple-foundation-models \
BANDICOT_PROFILE_SOURCE=$PACKAGE_ROOT/share/bandicot/openai.toml \
    "$REPO_ROOT/scripts/install-bandicot.sh"

printf '%s\n' 'Remote build installed. No Cargo build output was created locally.'
