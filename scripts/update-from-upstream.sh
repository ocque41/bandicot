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

case ${1:-} in
    '' ) ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/update-from-upstream.sh

Safely merge xai-org/grok-build main into this fork. A temporary candidate
worktree is validated and release-built before origin/main, local main, or the
installed grok-openai binary changes. This command never force-pushes, rebases,
stashes, resets, or pushes to upstream.
EOF
        exit 0
        ;;
    *) openai_workflow_die "unknown argument: $1" ;;
esac

cd "$REPO_ROOT"

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
[ -z "$(git status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'main must be completely clean, including untracked files'
[ -n "$(git config user.name 2>/dev/null || true)" ] || \
    openai_workflow_die 'Git user.name is required to create the upstream merge commit'
[ -n "$(git config user.email 2>/dev/null || true)" ] || \
    openai_workflow_die 'Git user.email is required to create the upstream merge commit'

DEFAULT_ORIGIN_HTTPS=https://github.com/ocque41/grok-build.git
DEFAULT_ORIGIN_SSH=git@github.com:ocque41/grok-build.git
DEFAULT_ORIGIN_SSH_URL=ssh://git@github.com/ocque41/grok-build.git
DEFAULT_UPSTREAM_HTTPS=https://github.com/xai-org/grok-build.git
DEFAULT_UPSTREAM_SSH=git@github.com:xai-org/grok-build.git
DEFAULT_UPSTREAM_SSH_URL=ssh://git@github.com/xai-org/grok-build.git

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
    openai_workflow_die "origin is not the ocque41/grok-build fork: $ORIGIN_URLS"

ORIGIN_PUSH_URLS=$(git config --get-all remote.origin.pushurl 2>/dev/null || true)
if [ -n "$ORIGIN_PUSH_URLS" ]; then
    case $ORIGIN_PUSH_URLS in
        *'
'*) openai_workflow_die 'origin must have at most one explicit push URL' ;;
    esac
    origin_url_allowed "$ORIGIN_PUSH_URLS" || \
        openai_workflow_die "origin push URL is not the ocque41/grok-build fork: $ORIGIN_PUSH_URLS"
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
PREVIOUS_UPSTREAM_COMMIT=$(git merge-base HEAD refs/remotes/upstream/main 2>/dev/null || true)
[ -n "$PREVIOUS_UPSTREAM_COMMIT" ] || \
    openai_workflow_die 'fork and upstream histories are unrelated; refusing an automatic merge'

if git merge-base --is-ancestor refs/remotes/upstream/main HEAD; then
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
            printf '%s\n' 'Local main, origin/main, and the installed grok-openai were not changed.' >&2
        else
            printf '%s\n' 'The validated merge was published, but installation did not finish.' >&2
            printf '%s\n' 'Re-run scripts/install-openai.sh from main after resolving the local error.' >&2
        fi
    fi
    exit "$_ow_rc"
}
trap report_retained_candidate 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

git worktree add -b "$CANDIDATE_BRANCH" "$CANDIDATE" "$BASE_COMMIT" >/dev/null

openai_workflow_note "Merging upstream/main in candidate: $CANDIDATE"
if ! git -C "$CANDIDATE" merge --no-edit --no-ff refs/remotes/upstream/main; then
    printf '%s\n' 'error: upstream merge conflicted; the conflict is isolated in the retained candidate' >&2
    exit 1
fi

if [ -n "${GROK_OPENAI_GATE_CMD:-}" ]; then
    openai_workflow_note 'Running explicit hermetic candidate gate...'
    if ! (cd "$CANDIDATE" && /bin/sh -c "$GROK_OPENAI_GATE_CMD"); then
        printf '%s\n' 'error: candidate validation gate failed' >&2
        exit 1
    fi
else
    openai_workflow_note 'Running canonical candidate validation and release build...'
    if ! CARGO_TARGET_DIR=$CANDIDATE_TARGET "$CANDIDATE/scripts/validate-openai.sh"; then
        printf '%s\n' 'error: candidate validation or release build failed' >&2
        exit 1
    fi
fi

if [ "$WORKFLOW_TEST_MODE" = 1 ] && [ -n "${GROK_OPENAI_PREBUILT:-}" ]; then
    RELEASE_BINARY=$(openai_workflow_abspath "$GROK_OPENAI_PREBUILT") || \
        openai_workflow_die "cannot resolve prebuilt binary: $GROK_OPENAI_PREBUILT"
else
    RELEASE_BINARY=$CANDIDATE_TARGET/release-dist/xai-grok-pager
fi
[ -x "$RELEASE_BINARY" ] || openai_workflow_die "validated release binary is missing: $RELEASE_BINARY"
"$RELEASE_BINARY" --version >/dev/null || openai_workflow_die 'validated release binary failed --version'

# Exercise the exact installer and launcher against a disposable HOME before
# any branch, remote, or live installation changes.
STAGE_HOME=$TEMP_ROOT/stage-home
mkdir -p "$STAGE_HOME"
if ! HOME=$STAGE_HOME \
    GROK_OPENAI_HOME=$STAGE_HOME/.grok-openai \
    GROK_OPENAI_BIN_DIR=$STAGE_HOME/.local/bin \
    GROK_OPENAI_LIBEXEC_DIR=$STAGE_HOME/.local/libexec/grok-openai \
    GROK_OPENAI_PROFILE_SOURCE=$CANDIDATE/config/openai.toml \
    GROK_OPENAI_PREBUILT=$RELEASE_BINARY \
    "$CANDIDATE/scripts/install-openai.sh" >/dev/null; then
    printf '%s\n' 'error: staged installation gate failed' >&2
    exit 1
fi

[ -z "$(git -C "$CANDIDATE" status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'candidate gates modified tracked files or created untracked repository files'

# Close the long-running validation race before publishing. If the user or
# another process changed main, stop while all durable state is still intact.
git fetch origin
[ "$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)" = main ] || \
    openai_workflow_die 'main checkout changed during validation'
[ -z "$(git status --porcelain --untracked-files=all)" ] || \
    openai_workflow_die 'main became dirty during validation'
[ "$(git rev-parse HEAD)" = "$BASE_COMMIT" ] || \
    openai_workflow_die 'local main advanced during validation'
[ "$(git rev-parse refs/remotes/origin/main)" = "$BASE_COMMIT" ] || \
    openai_workflow_die 'origin/main advanced during validation; rerun to merge the new state'

CANDIDATE_COMMIT=$(git -C "$CANDIDATE" rev-parse HEAD)
openai_workflow_note 'Publishing the validated fast-forward to origin/main...'
git -C "$CANDIDATE" push origin HEAD:refs/heads/main
PUBLISHED=1

git merge --ff-only "$CANDIDATE_COMMIT"

openai_workflow_note 'Installing the validated OpenAI build...'
GROK_OPENAI_PROFILE_SOURCE=$REPO_ROOT/config/openai.toml \
GROK_OPENAI_PREBUILT=$RELEASE_BINARY \
    "$REPO_ROOT/scripts/install-openai.sh"

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
openai_workflow_note 'origin/main, local main, and grok-openai now use the validated merge.'
