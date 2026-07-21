#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null) || {
    printf '%s\n' 'error: update-from-upstream.sh must run from a Git checkout' >&2
    exit 1
}
# shellcheck source=scripts/lib/openai-workflow.sh
. "$SCRIPT_DIR/lib/openai-workflow.sh"

ACCEPT_UPSTREAM_REWRITE=
case ${1:-} in
    '' ) ;;
    --accept-upstream-rewrite=*)
        ACCEPT_UPSTREAM_REWRITE=${1#--accept-upstream-rewrite=}
        [ -n "$ACCEPT_UPSTREAM_REWRITE" ] || \
            openai_workflow_die '--accept-upstream-rewrite requires <previous-sha>..<fetched-sha>'
        shift
        [ "$#" -eq 0 ] || openai_workflow_die "unknown argument: $1"
        ;;
    --accept-upstream-rewrite)
        openai_workflow_die '--accept-upstream-rewrite requires <previous-sha>..<fetched-sha>'
        ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/update-from-upstream.sh [--accept-upstream-rewrite=<previous-sha>..<fetched-sha>]

Safely merge xai-org/grok-build main into this fork. A temporary candidate
worktree is validated and release-built before origin/main, local main, or the
installed Bandicot binary changes. This command never force-pushes, rebases,
stashes, resets, or pushes to upstream.

The pinned --accept-upstream-rewrite option is required only when fetched
upstream/main no longer descends from the tracked last-integrated snapshot. The
refusal prints the exact pair required to accept that inspected snapshot.
EOF
        exit 0
        ;;
    *) openai_workflow_die "unknown argument: $1" ;;
esac

cd "$REPO_ROOT"
export GIT_NO_REPLACE_OBJECTS=1

WORKFLOW_TEST_MODE=${GROK_OPENAI_WORKFLOW_TEST_MODE:-0}
case $WORKFLOW_TEST_MODE in
    0|1) ;;
    *) openai_workflow_die 'GROK_OPENAI_WORKFLOW_TEST_MODE must be 0 or 1' ;;
esac
if [ "$WORKFLOW_TEST_MODE" != 1 ] && {
    [ -n "${GROK_OPENAI_GATE_CMD:-}" ] ||
    [ -n "${GROK_OPENAI_PREBUILT:-}" ] ||
    [ -n "${GROK_OPENAI_EXPECTED_ORIGIN_URL:-}" ] ||
    [ -n "${GROK_OPENAI_EXPECTED_UPSTREAM_URL:-}" ];
}; then
    openai_workflow_die 'test-only workflow overrides require GROK_OPENAI_WORKFLOW_TEST_MODE=1'
fi

CURRENT_BRANCH=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
[ "$CURRENT_BRANCH" = main ] || openai_workflow_die "run this command on main (current branch: ${CURRENT_BRANCH:-detached})"
[ -n "${HOME:-}" ] || openai_workflow_die 'HOME is required for candidate tooling and installation'
[ "$(git rev-parse --is-shallow-repository)" = false ] || \
    openai_workflow_die 'shallow repositories cannot prove upstream marker ancestry'
[ -z "$(git for-each-ref --format='%(refname)' refs/replace/)" ] || \
    openai_workflow_die 'Git replace refs are not allowed during an upstream update'
GIT_GRAFTS_PATH=$(git rev-parse --git-path info/grafts)
[ ! -s "$GIT_GRAFTS_PATH" ] || \
    openai_workflow_die 'Git grafts are not allowed during an upstream update'
[ -z "$(git status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'main must be completely clean, including untracked files'
[ -n "$(git config user.name 2>/dev/null || true)" ] || \
    openai_workflow_die 'Git user.name is required to create the upstream merge commit'
[ -n "$(git config user.email 2>/dev/null || true)" ] || \
    openai_workflow_die 'Git user.email is required to create the upstream merge commit'

DEFAULT_ORIGIN_HTTPS=https://github.com/ocque41/bandicot.git
DEFAULT_ORIGIN_SSH=git@github.com:ocque41/bandicot.git
DEFAULT_ORIGIN_SSH_URL=ssh://git@github.com/ocque41/bandicot.git
DEFAULT_UPSTREAM_HTTPS=https://github.com/xai-org/grok-build.git
DEFAULT_UPSTREAM_SSH=git@github.com:xai-org/grok-build.git
DEFAULT_UPSTREAM_SSH_URL=ssh://git@github.com/xai-org/grok-build.git
UPSTREAM_MARKER_PATH=.grok-openai-upstream

origin_url_allowed() {
    if [ -n "${GROK_OPENAI_EXPECTED_ORIGIN_URL:-}" ]; then
        [ "$1" = "$GROK_OPENAI_EXPECTED_ORIGIN_URL" ]
        return
    fi
    case $1 in
        "$DEFAULT_ORIGIN_HTTPS"|"$DEFAULT_ORIGIN_SSH"|"$DEFAULT_ORIGIN_SSH_URL") return 0 ;;
        *) return 1 ;;
    esac
}

upstream_url_allowed() {
    if [ -n "${GROK_OPENAI_EXPECTED_UPSTREAM_URL:-}" ]; then
        [ "$1" = "$GROK_OPENAI_EXPECTED_UPSTREAM_URL" ]
        return
    fi
    case $1 in
        "$DEFAULT_UPSTREAM_HTTPS"|"$DEFAULT_UPSTREAM_SSH"|"$DEFAULT_UPSTREAM_SSH_URL") return 0 ;;
        *) return 1 ;;
    esac
}

ORIGIN_URLS=$(git config --get-all remote.origin.url 2>/dev/null || true)
[ -n "$ORIGIN_URLS" ] || openai_workflow_die 'origin remote is missing'
case $ORIGIN_URLS in
    *'
'*) openai_workflow_die 'origin must have exactly one fetch URL' ;;
esac
origin_url_allowed "$ORIGIN_URLS" || \
    openai_workflow_die "origin is not the ocque41/bandicot fork: $ORIGIN_URLS"

ORIGIN_PUSH_URLS=$(git config --get-all remote.origin.pushurl 2>/dev/null || true)
if [ -n "$ORIGIN_PUSH_URLS" ]; then
    case $ORIGIN_PUSH_URLS in
        *'
'*) openai_workflow_die 'origin must have at most one explicit push URL' ;;
    esac
    origin_url_allowed "$ORIGIN_PUSH_URLS" || \
        openai_workflow_die "origin push URL is not the ocque41/bandicot fork: $ORIGIN_PUSH_URLS"
fi

if ! git config --get remote.upstream.url >/dev/null 2>&1; then
    UPSTREAM_TO_ADD=${GROK_OPENAI_EXPECTED_UPSTREAM_URL:-$DEFAULT_UPSTREAM_HTTPS}
    upstream_url_allowed "$UPSTREAM_TO_ADD" || openai_workflow_die "refusing unexpected upstream URL: $UPSTREAM_TO_ADD"
    git remote add upstream "$UPSTREAM_TO_ADD"
    openai_workflow_note "Added read-only upstream fetch remote: $UPSTREAM_TO_ADD"
fi

UPSTREAM_URLS=$(git config --get-all remote.upstream.url 2>/dev/null || true)
case $UPSTREAM_URLS in
    '') openai_workflow_die 'upstream fetch URL is missing' ;;
    *'
'*) openai_workflow_die 'upstream must have exactly one fetch URL' ;;
esac
upstream_url_allowed "$UPSTREAM_URLS" || \
    openai_workflow_die "unexpected upstream fetch URL: $UPSTREAM_URLS"

# Upstream is fetch-only. Replacing only its pushurl is an intentional safety
# setting; the origin push URL is never changed.
git config --unset-all remote.upstream.pushurl >/dev/null 2>&1 || true
git config remote.upstream.pushurl DISABLED
[ "$(git remote get-url --push upstream)" = DISABLED ] || \
    openai_workflow_die 'failed to disable upstream pushes'

openai_workflow_note 'Fetching origin and upstream...'
git fetch --prune origin
git fetch --prune upstream

git show-ref --verify --quiet refs/remotes/origin/main || openai_workflow_die 'origin/main does not exist'
git show-ref --verify --quiet refs/remotes/upstream/main || openai_workflow_die 'upstream/main does not exist'

BASE_COMMIT=$(git rev-parse HEAD)
ORIGIN_COMMIT=$(git rev-parse refs/remotes/origin/main)
UPSTREAM_COMMIT=$(git rev-parse refs/remotes/upstream/main)
[ "$BASE_COMMIT" = "$ORIGIN_COMMIT" ] || \
    openai_workflow_die 'local main must exactly match origin/main before updating'

# A force rewrite can remove every graph connection to the fork. The tracked
# marker is therefore the authoritative last upstream snapshot integrated into
# this append-only fork history. Refuse to infer or repair it: an invalid marker
# would make a synthetic history bridge apply an unreviewed tree delta.
git ls-files --error-unmatch -- "$UPSTREAM_MARKER_PATH" >/dev/null 2>&1 || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH must be tracked by the fork"
MARKER_MODE=$(git ls-tree HEAD -- "$UPSTREAM_MARKER_PATH" | awk 'NR == 1 { print $1 }')
[ "$MARKER_MODE" = 100644 ] || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH must be a non-executable regular tracked file"
MARKER_LINE_COUNT=$(git show "HEAD:$UPSTREAM_MARKER_PATH" | awk 'END { print NR }')
[ "$MARKER_LINE_COUNT" = 1 ] || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH must contain exactly one full lowercase commit SHA"
PREVIOUS_UPSTREAM_COMMIT=$(git show "HEAD:$UPSTREAM_MARKER_PATH" | sed -n '1p')
case $PREVIOUS_UPSTREAM_COMMIT in
    ''|*[!0-9a-f]*)
        openai_workflow_die "$UPSTREAM_MARKER_PATH must contain exactly one full lowercase commit SHA"
        ;;
esac
RESOLVED_MARKER_COMMIT=$(git rev-parse --verify "$PREVIOUS_UPSTREAM_COMMIT^{commit}" 2>/dev/null) || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH does not identify a valid commit"
[ "$PREVIOUS_UPSTREAM_COMMIT" = "$RESOLVED_MARKER_COMMIT" ] || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH must contain exactly one full lowercase commit SHA"
git merge-base --is-ancestor "$PREVIOUS_UPSTREAM_COMMIT" HEAD || \
    openai_workflow_die "$UPSTREAM_MARKER_PATH commit is not an ancestor of fork HEAD"
if git cat-file -e "$UPSTREAM_COMMIT:$UPSTREAM_MARKER_PATH" 2>/dev/null; then
    openai_workflow_die "upstream/main contains the fork-reserved $UPSTREAM_MARKER_PATH path"
fi

if git merge-base --is-ancestor "$PREVIOUS_UPSTREAM_COMMIT" "$UPSTREAM_COMMIT"; then
    UPSTREAM_REWRITE=0
else
    UPSTREAM_REWRITE=1
fi

EXPECTED_REWRITE_ACCEPTANCE=$PREVIOUS_UPSTREAM_COMMIT..$UPSTREAM_COMMIT
if [ "$UPSTREAM_REWRITE" = 1 ] && [ "$ACCEPT_UPSTREAM_REWRITE" != "$EXPECTED_REWRITE_ACCEPTANCE" ]; then
    openai_workflow_die "upstream/main no longer descends from $UPSTREAM_MARKER_PATH; inspect the fetched history, then rerun with --accept-upstream-rewrite=$EXPECTED_REWRITE_ACCEPTANCE"
fi
if [ "$UPSTREAM_REWRITE" = 0 ] && [ -n "$ACCEPT_UPSTREAM_REWRITE" ]; then
    openai_workflow_die '--accept-upstream-rewrite is valid only when upstream no longer descends from the marker'
fi

if [ "$UPSTREAM_REWRITE" = 0 ] && git merge-base --is-ancestor refs/remotes/upstream/main HEAD; then
    openai_workflow_note 'Already up to date: upstream/main is contained in this fork.'
    openai_workflow_note "Fork main: $BASE_COMMIT"
    openai_workflow_note "Upstream main: $UPSTREAM_COMMIT"
    exit 0
fi

TMP_PARENT=${TMPDIR:-/tmp}
mkdir -p "$TMP_PARENT"
TEMP_ROOT=$(mktemp -d "$TMP_PARENT/grok-openai-update.XXXXXX") || \
    openai_workflow_die 'failed to create candidate directory'
CANDIDATE=$TEMP_ROOT/candidate
CANDIDATE_TARGET=$TEMP_ROOT/cargo-target
CANDIDATE_BRANCH=openai-update-$(date +%Y%m%d%H%M%S)-$$
SUCCESS=0
PUBLISHED=0

report_retained_candidate() {
    _ow_rc=$?
    if [ "$SUCCESS" -eq 1 ]; then
        exit "$_ow_rc"
    fi
    if [ -d "$CANDIDATE" ]; then
        printf '\nCandidate retained for inspection: %s\n' "$CANDIDATE" >&2
        printf 'Candidate branch: %s\n' "$CANDIDATE_BRANCH" >&2
        printf 'Inspect with: git -C %s status\n' "$CANDIDATE" >&2
        if [ "$PUBLISHED" -eq 0 ]; then
            printf '%s\n' 'Local main, origin/main, and the installed Bandicot were not changed.' >&2
        else
            printf '%s\n' 'The validated merge was published, but installation did not finish.' >&2
            printf '%s\n' 'Re-run scripts/install-bandicot.sh from main after resolving the local error.' >&2
        fi
    fi
    exit "$_ow_rc"
}
trap report_retained_candidate 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

git worktree add -b "$CANDIDATE_BRANCH" "$CANDIDATE" "$BASE_COMMIT" >/dev/null

MERGE_TARGET=$UPSTREAM_COMMIT
if [ "$UPSTREAM_REWRITE" = 1 ]; then
    openai_workflow_note 'Accepted rewritten, rebased, or rolled-back upstream history; synthesizing an append-only bridge in the candidate.'
    UPSTREAM_TREE=$(git -C "$CANDIDATE" rev-parse "$UPSTREAM_COMMIT^{tree}")
    BRIDGE_COMMIT=$(
        printf '%s\n\n%s\n%s\n%s\n' \
            'Bridge force-rewritten upstream/main' \
            'This synthetic commit carries the fetched upstream tree.' \
            "Previously integrated upstream: $PREVIOUS_UPSTREAM_COMMIT" \
            "Fetched rewritten upstream: $UPSTREAM_COMMIT" |
            git -C "$CANDIDATE" commit-tree "$UPSTREAM_TREE" \
                -p "$PREVIOUS_UPSTREAM_COMMIT" -p "$UPSTREAM_COMMIT"
    ) || openai_workflow_die 'failed to create rewritten-upstream bridge in the candidate'
    [ "$(git -C "$CANDIDATE" rev-parse "$BRIDGE_COMMIT^1")" = "$PREVIOUS_UPSTREAM_COMMIT" ] || \
        openai_workflow_die 'rewritten-upstream bridge has an invalid first parent'
    [ "$(git -C "$CANDIDATE" rev-parse "$BRIDGE_COMMIT^2")" = "$UPSTREAM_COMMIT" ] || \
        openai_workflow_die 'rewritten-upstream bridge has an invalid second parent'
    [ "$(git -C "$CANDIDATE" rev-parse "$BRIDGE_COMMIT^{tree}")" = "$UPSTREAM_TREE" ] || \
        openai_workflow_die 'rewritten-upstream bridge does not carry the fetched upstream tree'
    MERGE_TARGET=$BRIDGE_COMMIT
fi

openai_workflow_note "Merging upstream/main in candidate: $CANDIDATE"
if ! git -C "$CANDIDATE" merge --no-commit --no-ff "$MERGE_TARGET"; then
    printf '%s\n' 'error: upstream merge conflicted; the conflict is isolated in the retained candidate' >&2
    exit 1
fi
printf '%s\n' "$UPSTREAM_COMMIT" >"$CANDIDATE/$UPSTREAM_MARKER_PATH"
git -C "$CANDIDATE" add -- "$UPSTREAM_MARKER_PATH"
git -C "$CANDIDATE" -c core.hooksPath=/dev/null -c commit.gpgSign=false commit --no-verify \
    -m "Merge upstream/main at $UPSTREAM_COMMIT" >/dev/null
EXPECTED_CANDIDATE_COMMIT=$(git -C "$CANDIDATE" rev-parse HEAD)

verify_candidate_commit() {
    [ "$(git -C "$CANDIDATE" symbolic-ref --quiet --short HEAD 2>/dev/null || true)" = "$CANDIDATE_BRANCH" ] || \
        openai_workflow_die 'candidate branch changed during validation'
    [ "$(git -C "$CANDIDATE" rev-parse HEAD)" = "$EXPECTED_CANDIDATE_COMMIT" ] || \
        openai_workflow_die 'candidate HEAD changed during validation'
    [ "$(git rev-parse "refs/heads/$CANDIDATE_BRANCH")" = "$EXPECTED_CANDIDATE_COMMIT" ] || \
        openai_workflow_die 'candidate branch ref changed during validation'

    CANDIDATE_PARENT_COUNT=$(git cat-file -p "$EXPECTED_CANDIDATE_COMMIT" | \
        awk '$1 == "parent" { count++ } END { print count + 0 }')
    [ "$CANDIDATE_PARENT_COUNT" -eq 2 ] || \
        openai_workflow_die 'candidate merge commit must have exactly two parents'
    [ "$(git rev-parse "$EXPECTED_CANDIDATE_COMMIT^1")" = "$BASE_COMMIT" ] || \
        openai_workflow_die 'candidate merge has an invalid first parent'
    [ "$(git rev-parse "$EXPECTED_CANDIDATE_COMMIT^2")" = "$MERGE_TARGET" ] || \
        openai_workflow_die 'candidate merge has an invalid second parent'
    [ "$(git ls-tree "$EXPECTED_CANDIDATE_COMMIT" -- "$UPSTREAM_MARKER_PATH" | awk 'NR == 1 { print $1 }')" = 100644 ] || \
        openai_workflow_die 'candidate merge has an invalid upstream marker mode'
    [ "$(git show "$EXPECTED_CANDIDATE_COMMIT:$UPSTREAM_MARKER_PATH" | awk 'END { print NR }')" = 1 ] || \
        openai_workflow_die 'candidate merge has an invalid upstream marker line count'
    [ "$(git show "$EXPECTED_CANDIDATE_COMMIT:$UPSTREAM_MARKER_PATH" | sed -n '1p')" = "$UPSTREAM_COMMIT" ] || \
        openai_workflow_die 'candidate merge does not record the fetched upstream commit'
    git merge-base --is-ancestor "$UPSTREAM_COMMIT" "$EXPECTED_CANDIDATE_COMMIT" || \
        openai_workflow_die 'fetched upstream is not an ancestor of the candidate merge'

    [ -z "$(git for-each-ref --format='%(refname)' refs/replace/)" ] || \
        openai_workflow_die 'candidate gates created a Git replace ref'
    [ ! -s "$GIT_GRAFTS_PATH" ] || openai_workflow_die 'candidate gates created a Git graft'
}

verify_candidate_commit

VALIDATION_HOME=$TEMP_ROOT/validation-home
VALIDATION_TMP=$TEMP_ROOT/validation-tmp
VALIDATION_CARGO_HOME=${CARGO_HOME:-${HOME:-}/.cargo}
VALIDATION_RUSTUP_HOME=${RUSTUP_HOME:-${HOME:-}/.rustup}
mkdir -p "$VALIDATION_HOME" "$VALIDATION_TMP"

# Candidate source and build scripts run with the caller's filesystem
# permissions, but not with ambient credentials in their process environment.
# A disposable HOME also prevents implicit use of user Git/auth configuration.
# Cargo and rustup roots remain explicit so the existing public toolchain and
# dependency cache can be used; see docs/UPDATING.md for the remaining trust
# boundary before accepting upstream source.
if [ -n "${GROK_OPENAI_GATE_CMD:-}" ]; then
    openai_workflow_note 'Running explicit hermetic candidate gate...'
    if ! (cd "$CANDIDATE" && env -i \
        HOME="$VALIDATION_HOME" \
        PATH="${PATH:-/usr/bin:/bin}" \
        TMPDIR="$VALIDATION_TMP" \
        CARGO_HOME="$VALIDATION_CARGO_HOME" \
        RUSTUP_HOME="$VALIDATION_RUSTUP_HOME" \
        CARGO_TARGET_DIR="$CANDIDATE_TARGET" \
        LC_ALL=C \
        /bin/sh -c "$GROK_OPENAI_GATE_CMD"); then
        printf '%s\n' 'error: candidate validation gate failed' >&2
        exit 1
    fi
else
    openai_workflow_note 'Running canonical candidate validation and release build...'
    if ! env -i \
        HOME="$VALIDATION_HOME" \
        PATH="${PATH:-/usr/bin:/bin}" \
        TMPDIR="$VALIDATION_TMP" \
        CARGO_HOME="$VALIDATION_CARGO_HOME" \
        RUSTUP_HOME="$VALIDATION_RUSTUP_HOME" \
        CARGO_TARGET_DIR="$CANDIDATE_TARGET" \
        LC_ALL=C \
        "$CANDIDATE/scripts/validate-openai.sh"; then
        printf '%s\n' 'error: candidate validation or release build failed' >&2
        exit 1
    fi
fi

verify_candidate_commit

if [ "$WORKFLOW_TEST_MODE" = 1 ] && [ -n "${GROK_OPENAI_PREBUILT:-}" ]; then
    RELEASE_BINARY=$(openai_workflow_abspath "$GROK_OPENAI_PREBUILT") || \
        openai_workflow_die "cannot resolve prebuilt binary: $GROK_OPENAI_PREBUILT"
else
    RELEASE_BINARY=$CANDIDATE_TARGET/release-dist/xai-grok-pager
fi
[ -x "$RELEASE_BINARY" ] || openai_workflow_die "validated release binary is missing: $RELEASE_BINARY"
env -i \
    HOME="$VALIDATION_HOME" \
    PATH="${PATH:-/usr/bin:/bin}" \
    TMPDIR="$VALIDATION_TMP" \
    LC_ALL=C \
    "$RELEASE_BINARY" --version >/dev/null || \
    openai_workflow_die 'validated release binary failed --version'

# Exercise the exact installer and launcher against a disposable HOME before
# any branch, remote, or live installation changes.
STAGE_HOME=$TEMP_ROOT/stage-home
mkdir -p "$STAGE_HOME"
if ! env -i \
    HOME="$STAGE_HOME" \
    PATH="${PATH:-/usr/bin:/bin}" \
    TMPDIR="$VALIDATION_TMP" \
    LC_ALL=C \
    BANDICOT_HOME="$STAGE_HOME/.bandicot" \
    BANDICOT_BIN_DIR="$STAGE_HOME/.local/bin" \
    BANDICOT_LIBEXEC_DIR="$STAGE_HOME/.local/libexec/bandicot" \
    BANDICOT_PROFILE_SOURCE="$CANDIDATE/config/openai.toml" \
    BANDICOT_PREBUILT="$RELEASE_BINARY" \
    "$CANDIDATE/scripts/install-bandicot.sh" >/dev/null; then
    printf '%s\n' 'error: staged installation gate failed' >&2
    exit 1
fi

[ -z "$(git -C "$CANDIDATE" status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'candidate gates modified tracked files or created untracked repository files'
verify_candidate_commit

[ "$(git config --get-all remote.origin.url 2>/dev/null || true)" = "$ORIGIN_URLS" ] || \
    openai_workflow_die 'origin fetch URL changed during validation'
[ "$(git config --get-all remote.origin.pushurl 2>/dev/null || true)" = "$ORIGIN_PUSH_URLS" ] || \
    openai_workflow_die 'origin push URL changed during validation'
[ "$(git config --get-all remote.upstream.url 2>/dev/null || true)" = "$UPSTREAM_URLS" ] || \
    openai_workflow_die 'upstream fetch URL changed during validation'
[ "$(git remote get-url --push upstream)" = DISABLED ] || \
    openai_workflow_die 'upstream push protection changed during validation'

# Close the long-running validation race before publishing. If the user or
# another process changed main, stop while all durable state is still intact.
git fetch origin
git fetch upstream
[ "$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)" = main ] || \
    openai_workflow_die 'main checkout changed during validation'
[ -z "$(git status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'main became dirty during validation'
[ "$(git rev-parse HEAD)" = "$BASE_COMMIT" ] || \
    openai_workflow_die 'local main advanced during validation'
[ "$(git rev-parse refs/remotes/origin/main)" = "$BASE_COMMIT" ] || \
    openai_workflow_die 'origin/main advanced during validation; rerun to merge the new state'
[ "$(git rev-parse refs/remotes/upstream/main)" = "$UPSTREAM_COMMIT" ] || \
    openai_workflow_die 'upstream/main changed during validation; rerun against the newly fetched snapshot'

CANDIDATE_COMMIT=$EXPECTED_CANDIDATE_COMMIT
openai_workflow_note 'Publishing the validated fast-forward to origin/main...'
git -C "$CANDIDATE" -c core.hooksPath=/dev/null push origin \
    "$EXPECTED_CANDIDATE_COMMIT:refs/heads/main"
PUBLISHED=1

git -c core.hooksPath=/dev/null merge --ff-only "$CANDIDATE_COMMIT"

openai_workflow_note 'Installing the validated OpenAI build...'
LIVE_BANDICOT_HOME=${BANDICOT_HOME:-$HOME/.bandicot}
LIVE_BANDICOT_BIN_DIR=${BANDICOT_BIN_DIR:-$HOME/.local/bin}
LIVE_BANDICOT_LIBEXEC_DIR=${BANDICOT_LIBEXEC_DIR:-$HOME/.local/libexec/bandicot}
env -i \
HOME="$HOME" \
PATH="${PATH:-/usr/bin:/bin}" \
TMPDIR="${TMPDIR:-/tmp}" \
LC_ALL=C \
    BANDICOT_HOME="$LIVE_BANDICOT_HOME" \
    BANDICOT_BIN_DIR="$LIVE_BANDICOT_BIN_DIR" \
    BANDICOT_LIBEXEC_DIR="$LIVE_BANDICOT_LIBEXEC_DIR" \
    BANDICOT_PROFILE_SOURCE="$REPO_ROOT/config/openai.toml" \
    BANDICOT_PREBUILT="$RELEASE_BINARY" \
    "$REPO_ROOT/scripts/install-bandicot.sh"

SUCCESS=1
trap - 0
trap - HUP INT TERM
git worktree remove "$CANDIDATE"
git branch -d "$CANDIDATE_BRANCH" >/dev/null
rm -rf "$TEMP_ROOT"

openai_workflow_note "Update complete: $(git rev-parse --short HEAD)"
openai_workflow_note "Fork before: $BASE_COMMIT"
openai_workflow_note "Previously integrated upstream: $PREVIOUS_UPSTREAM_COMMIT"
openai_workflow_note "Fetched upstream: $UPSTREAM_COMMIT"
openai_workflow_note "Fork after: $CANDIDATE_COMMIT"
openai_workflow_note 'origin/main, local main, and Bandicot now use the validated merge.'
