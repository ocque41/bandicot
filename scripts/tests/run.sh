#!/bin/sh
set -eu
umask 077

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd -P)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/grok-openai-script-tests.XXXXXX")
mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/git-template" "$TEST_ROOT/tmp"

# Every child, including Git itself, is isolated from the user's real HOME,
# global Git config, and template hooks. Every configured remote is a local
# bare repository beneath TEST_ROOT, so these tests never access the network.
export HOME="$TEST_ROOT/home"
export TMPDIR="$TEST_ROOT/tmp"
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_TEMPLATE_DIR="$TEST_ROOT/git-template"

cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_file() {
    [ -f "$1" ] || fail "expected file: $1"
}

assert_absent() {
    [ ! -e "$1" ] || fail "expected path to remain absent: $1"
}

assert_eq() {
    [ "$1" = "$2" ] || fail "expected '$1' to equal '$2'"
}

make_fake_binary() {
    _test_binary=$1
    mkdir -p "$(dirname -- "$_test_binary")"
    cat >"$_test_binary" <<'EOF'
#!/bin/sh
[ -z "${CANDIDATE_SECRET_SENTINEL:-}" ] || {
    printf '%s\n' 'candidate binary inherited ambient sentinel secret' >&2
    exit 97
}

case ${1:-} in
    -v|-V|--version)
        printf '%s\n' 'grok-openai-test 1.0'
        exit 0
        ;;
esac
if [ -n "${GROK_OPENAI_TEST_CAPTURE:-}" ]; then
    {
        printf 'GROK_HOME=%s\n' "${GROK_HOME:-}"
        printf 'GROK_DISABLE_AUTOUPDATER=%s\n' "${GROK_DISABLE_AUTOUPDATER:-}"
        printf 'GROK_OPENAI_DISABLE_VENDOR_UPDATE=%s\n' "${GROK_OPENAI_DISABLE_VENDOR_UPDATE:-}"
        printf 'GROK_IMAGE_GEN=%s\n' "${GROK_IMAGE_GEN:-}"
        printf 'GROK_IMAGE_EDIT=%s\n' "${GROK_IMAGE_EDIT:-}"
        printf 'GROK_VOICE_MODE=%s\n' "${GROK_VOICE_MODE:-}"
        if [ -n "${OPENAI_API_KEY:-}" ]; then
            printf '%s\n' 'OPENAI_API_KEY_SET=1'
        else
            printf '%s\n' 'OPENAI_API_KEY_SET=0'
        fi
        if [ -n "${GROK_CODEX_PROXY_TOKEN:-}" ]; then
            printf '%s\n' 'GROK_CODEX_PROXY_TOKEN_SET=1'
        else
            printf '%s\n' 'GROK_CODEX_PROXY_TOKEN_SET=0'
        fi
        if [ -n "${XAI_API_KEY:-}${GROK_CODE_XAI_API_KEY:-}${GROK_AUTH:-}${GROK_AUTH_PATH:-}${GROK_AUTH_PROVIDER_COMMAND:-}" ]; then
            printf '%s\n' 'XAI_AUTH_INPUT_SET=1'
        else
            printf '%s\n' 'XAI_AUTH_INPUT_SET=0'
        fi
    } >"$GROK_OPENAI_TEST_CAPTURE"
fi
exit 0
EOF
    chmod 755 "$_test_binary"
}

write_profile() {
    _test_profile=$1
    mkdir -p "$(dirname -- "$_test_profile")"
    cat >"$_test_profile" <<'EOF'
[cli]
auto_update = false

[models]
default = "openai-latest"

[model.openai-latest]
model = "gpt-test"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
api_backend = "responses"
EOF
}

test_install_isolation() {
    _test_case=$TEST_ROOT/install
    _test_home=$_test_case/home
    _test_binary=$_test_case/fake-grok
    _test_profile=$_test_case/openai.toml
    _test_capture=$_test_case/capture
    mkdir -p "$_test_home/.grok"
    printf '%s\n' 'legacy-sentinel' >"$_test_home/.grok/config.toml"
    make_fake_binary "$_test_binary"
    write_profile "$_test_profile"

    HOME=$_test_home \
    GROK_OPENAI_PREBUILT=$_test_binary \
    GROK_OPENAI_PROFILE_SOURCE=$_test_profile \
        "$REPO_ROOT/scripts/install-openai.sh" >/dev/null

    assert_file "$_test_home/.local/libexec/grok-openai/grok"
    assert_file "$_test_home/.local/libexec/grok-openai/openai.toml"
    assert_file "$_test_home/.local/libexec/grok-openai/codex-plan.toml"
    assert_file "$_test_home/.local/bin/grok-openai"
    assert_file "$_test_home/.grok-openai/config.toml"
    assert_file "$_test_home/.grok-codex-plan/config.toml"
    assert_eq "$(sed -n '1p' "$_test_home/.grok/config.toml")" legacy-sentinel

    if HOME=$_test_home \
        GROK_OPENAI_HOME=$_test_home/.grok \
        GROK_OPENAI_PREBUILT=$_test_binary \
        GROK_OPENAI_PROFILE_SOURCE=$_test_profile \
            "$REPO_ROOT/scripts/install-openai.sh" >"$_test_case/legacy-refusal.out" 2>&1; then
        fail 'installer unexpectedly accepted the legacy ~/.grok directory'
    fi
    grep -q 'refusing to use or modify the legacy ~/.grok directory' "$_test_case/legacy-refusal.out" || fail 'legacy ~/.grok refusal was not actionable'
    assert_eq "$(sed -n '1p' "$_test_home/.grok/config.toml")" legacy-sentinel

    if HOME=$_test_home \
        GROK_OPENAI_HOME=$_test_home/.grok \
            "$_test_home/.local/bin/grok-openai" --version >"$_test_case/runtime-legacy-refusal.out" 2>&1; then
        fail 'launcher unexpectedly accepted the legacy ~/.grok directory'
    fi
    grep -q 'refusing to use the legacy ~/.grok directory' "$_test_case/runtime-legacy-refusal.out" || fail 'launcher legacy ~/.grok refusal was not actionable'

    if HOME=$_test_home \
        GROK_OPENAI_HOME=$_test_home/.grok-openai/../.grok \
            "$_test_home/.local/bin/grok-openai" --version >"$_test_case/runtime-traversal-refusal.out" 2>&1; then
        fail 'launcher unexpectedly accepted a path traversal into legacy ~/.grok'
    fi
    grep -q 'must not contain . or .. path components' "$_test_case/runtime-traversal-refusal.out" || fail 'launcher path traversal refusal was not actionable'
    assert_eq "$(sed -n '1p' "$_test_home/.grok/config.toml")" legacy-sentinel

    HOME=$_test_home \
    OPENAI_API_KEY=not-a-real-key \
    XAI_API_KEY=must-be-removed \
    GROK_CODE_XAI_API_KEY=must-be-removed \
    GROK_AUTH=must-be-removed \
    GROK_AUTH_PATH=$_test_case/must-not-be-read.json \
    GROK_AUTH_PROVIDER_COMMAND=must-be-removed \
    GROK_OPENAI_TEST_CAPTURE=$_test_capture \
        "$_test_home/.local/bin/grok-openai" probe

    grep -q "GROK_HOME=$_test_home/.grok-openai" "$_test_capture" || fail 'launcher did not isolate GROK_HOME'
    grep -q '^GROK_DISABLE_AUTOUPDATER=1$' "$_test_capture" || fail 'launcher did not disable updater'
    grep -q '^GROK_OPENAI_DISABLE_VENDOR_UPDATE=1$' "$_test_capture" || fail 'launcher did not disable explicit vendor updates'
    grep -q '^GROK_IMAGE_GEN=0$' "$_test_capture" || fail 'launcher did not disable image generation'
    grep -q '^GROK_IMAGE_EDIT=0$' "$_test_capture" || fail 'launcher did not disable image editing'
    grep -q '^GROK_VOICE_MODE=0$' "$_test_capture" || fail 'launcher did not disable voice'
    grep -q '^OPENAI_API_KEY_SET=1$' "$_test_capture" || fail 'launcher did not pass OpenAI key presence'
    grep -q '^XAI_AUTH_INPUT_SET=0$' "$_test_capture" || fail 'launcher did not clear ambient xAI auth inputs'
    if grep -q 'not-a-real-key' "$_test_capture"; then
        fail 'test capture leaked the API key value'
    fi

    # Static model discovery is intentionally offline and must work before a
    # Platform key has been configured. This branch also guarantees the test
    # never queries the user's real macOS Keychain.
    (
        unset OPENAI_API_KEY
        HOME=$_test_home \
        GROK_OPENAI_TEST_CAPTURE=$_test_capture \
            "$_test_home/.local/bin/grok-openai" models
    )
    grep -q '^OPENAI_API_KEY_SET=0$' "$_test_capture" || fail 'offline models command unexpectedly required a key'

    # Upstream leader management is local socket/process inspection. It must
    # remain usable before an OpenAI key is configured.
    (
        unset OPENAI_API_KEY
        HOME=$_test_home \
        GROK_OPENAI_TEST_CAPTURE=$_test_capture \
            "$_test_home/.local/bin/grok-openai" leader list --json
    )
    grep -q '^OPENAI_API_KEY_SET=0$' "$_test_capture" || fail 'local leader command unexpectedly required a key'

    # Without a Platform key, the launcher selects the Grok Build profile for
    # the local Codex OAuth proxy and reads only its protected client token.
    _test_proxy_token=$_test_case/client-token
    printf '%s\n' 'local-loopback-test-token' >"$_test_proxy_token"
    chmod 600 "$_test_proxy_token"
    (
        unset OPENAI_API_KEY
        HOME=$_test_home \
        GROK_CODEX_PROXY_TOKEN_FILE=$_test_proxy_token \
        GROK_OPENAI_TEST_CAPTURE=$_test_capture \
            "$_test_home/.local/bin/grok-openai" 'hello from bandicot' >/dev/null 2>&1
    )
    grep -q "GROK_HOME=$_test_home/.grok-codex-plan" "$_test_capture" || fail 'launcher did not select the Codex-plan profile'
    grep -q '^GROK_CODEX_PROXY_TOKEN_SET=1$' "$_test_capture" || fail 'launcher did not supply the local proxy token'
    grep -q '^OPENAI_API_KEY_SET=0$' "$_test_capture" || fail 'Codex-plan mode synthesized an OpenAI Platform key'
    if grep -q 'local-loopback-test-token' "$_test_capture"; then
        fail 'test capture leaked the local proxy token value'
    fi

    # An untouched generated profile follows installer upgrades, while a user
    # edit is preserved and the new canonical profile remains available.
    printf '%s\n' '# profile-v2' >>"$_test_profile"
    HOME=$_test_home \
    GROK_OPENAI_PREBUILT=$_test_binary \
    GROK_OPENAI_PROFILE_SOURCE=$_test_profile \
        "$REPO_ROOT/scripts/install-openai.sh" >/dev/null
    grep -q '^# profile-v2$' "$_test_home/.grok-openai/config.toml" || fail 'untouched generated profile did not update'

    printf '%s\n' '# user-customization' >>"$_test_home/.grok-openai/config.toml"
    printf '%s\n' '# profile-v3' >>"$_test_profile"
    HOME=$_test_home \
    GROK_OPENAI_PREBUILT=$_test_binary \
    GROK_OPENAI_PROFILE_SOURCE=$_test_profile \
        "$REPO_ROOT/scripts/install-openai.sh" >/dev/null
    grep -q '^# user-customization$' "$_test_home/.grok-openai/config.toml" || fail 'customized runtime profile was overwritten'
    if grep -q '^# profile-v3$' "$_test_home/.grok-openai/config.toml"; then
        fail 'customized runtime profile unexpectedly followed canonical update'
    fi
    grep -q '^# profile-v3$' "$_test_home/.local/libexec/grok-openai/openai.toml" || fail 'canonical profile did not update'

    OPENAI_API_KEY=not-a-real-key "$REPO_ROOT/scripts/setup-openai-key.sh" >/dev/null
}

make_fixture() {
    _test_name=$1
    FIXTURE=$TEST_ROOT/$_test_name
    SEED=$FIXTURE/seed
    ORIGIN=$FIXTURE/origin.git
    UPSTREAM=$FIXTURE/upstream.git
    WORK=$FIXTURE/work
    UPSTREAM_WORK=$FIXTURE/upstream-work
    HOME_DIR=$FIXTURE/home
    FAKE_BINARY=$FIXTURE/fake-grok

    mkdir -p "$FIXTURE" "$HOME_DIR" "$FIXTURE/tmp"
    git init --bare --initial-branch=main "$ORIGIN" >/dev/null
    git init --bare --initial-branch=main "$UPSTREAM" >/dev/null
    git init -b main "$SEED" >/dev/null
    git -C "$SEED" config user.name 'Script Test'
    git -C "$SEED" config user.email 'script-test@example.invalid'
    cp -R "$REPO_ROOT/scripts" "$SEED/scripts"
    write_profile "$SEED/config/openai.toml"
    cp "$REPO_ROOT/config/codex-plan.toml" "$SEED/config/codex-plan.toml"
    printf '%s\n' base >"$SEED/conflict.txt"
    printf '%s\n' base >"$SEED/base.txt"
    git -C "$SEED" add .
    git -C "$SEED" commit -m base >/dev/null
    UPSTREAM_BASE=$(git -C "$SEED" rev-parse HEAD)
    git -C "$SEED" remote add origin "$ORIGIN"
    git -C "$SEED" remote add upstream "$UPSTREAM"
    git -C "$SEED" push upstream main >/dev/null
    printf '%s\n' "$UPSTREAM_BASE" >"$SEED/.grok-openai-upstream"
    git -C "$SEED" add .grok-openai-upstream
    git -C "$SEED" commit -m 'track integrated upstream snapshot' >/dev/null
    git -C "$SEED" push origin main >/dev/null
    git --git-dir="$ORIGIN" symbolic-ref HEAD refs/heads/main
    git --git-dir="$UPSTREAM" symbolic-ref HEAD refs/heads/main

    git clone --branch main "$ORIGIN" "$WORK" >/dev/null 2>&1
    git -C "$WORK" config user.name 'Script Test'
    git -C "$WORK" config user.email 'script-test@example.invalid'
    git -C "$WORK" remote add upstream "$UPSTREAM"
    git clone --branch main "$UPSTREAM" "$UPSTREAM_WORK" >/dev/null 2>&1
    git -C "$UPSTREAM_WORK" config user.name 'Upstream Test'
    git -C "$UPSTREAM_WORK" config user.email 'upstream-test@example.invalid'
    make_fake_binary "$FAKE_BINARY"
}

# Model an upstream force rewrite without rewriting any fork history. The new
# root commit intentionally has no parents but reuses the current upstream
# index as its complete snapshot tree.
force_rewrite_upstream() {
    _test_rewritten_tree=$(git -C "$UPSTREAM_WORK" write-tree)
    REWRITTEN_UPSTREAM_COMMIT=$(
        printf '%s\n' 'rewritten upstream root' |
            git -C "$UPSTREAM_WORK" commit-tree "$_test_rewritten_tree"
    )
    git -C "$UPSTREAM_WORK" push --force origin \
        "$REWRITTEN_UPSTREAM_COMMIT:refs/heads/main" >/dev/null
}

rewrite_upstream_from_parent() {
    _test_rewritten_parent=$1
    _test_rewritten_tree=$(git -C "$UPSTREAM_WORK" write-tree)
    REWRITTEN_UPSTREAM_COMMIT=$(
        printf '%s\n' 'rebased upstream snapshot' |
            git -C "$UPSTREAM_WORK" commit-tree "$_test_rewritten_tree" \
                -p "$_test_rewritten_parent"
    )
    git -C "$UPSTREAM_WORK" push --force origin \
        "$REWRITTEN_UPSTREAM_COMMIT:refs/heads/main" >/dev/null
}

run_updater() {
    _test_output=$1
    shift
    (
        cd "$WORK"
        env \
            HOME="$HOME_DIR" \
            TMPDIR="$FIXTURE/tmp" \
            GROK_OPENAI_WORKFLOW_TEST_MODE=1 \
            GROK_OPENAI_EXPECTED_ORIGIN_URL="$ORIGIN" \
            GROK_OPENAI_EXPECTED_UPSTREAM_URL="$UPSTREAM" \
            GROK_OPENAI_PREBUILT="$FAKE_BINARY" \
            GROK_OPENAI_GATE_CMD="${GROK_OPENAI_GATE_CMD_TEST:-true}" \
            "$@" \
            ./scripts/update-from-upstream.sh
    ) >"$_test_output" 2>&1
}

run_updater_accept_pair() {
    _test_output=$1
    _test_accept_pair=$2
    shift 2
    (
        cd "$WORK"
        env \
            HOME="$HOME_DIR" \
            TMPDIR="$FIXTURE/tmp" \
            GROK_OPENAI_WORKFLOW_TEST_MODE=1 \
            GROK_OPENAI_EXPECTED_ORIGIN_URL="$ORIGIN" \
            GROK_OPENAI_EXPECTED_UPSTREAM_URL="$UPSTREAM" \
            GROK_OPENAI_PREBUILT="$FAKE_BINARY" \
            GROK_OPENAI_GATE_CMD="${GROK_OPENAI_GATE_CMD_TEST:-true}" \
            "$@" \
            ./scripts/update-from-upstream.sh \
                "--accept-upstream-rewrite=$_test_accept_pair"
    ) >"$_test_output" 2>&1
}

run_updater_accept_rewrite() {
    _test_output=$1
    shift
    _test_accept_pair=$(git -C "$WORK" show HEAD:.grok-openai-upstream)..$(git --git-dir="$UPSTREAM" rev-parse refs/heads/main)
    run_updater_accept_pair "$_test_output" "$_test_accept_pair" "$@"
}

test_bypasses_require_explicit_test_mode() {
    _test_output=$TEST_ROOT/bypass-refusal.out
    if GROK_OPENAI_GATE_CMD=true "$REPO_ROOT/scripts/validate-openai.sh" >"$_test_output" 2>&1; then
        fail 'validation bypass worked without explicit workflow test mode'
    fi
    grep -q 'requires GROK_OPENAI_WORKFLOW_TEST_MODE=1' "$_test_output" || fail 'validation bypass refusal was not actionable'

    make_fixture bypass-refusal
    if (
        cd "$WORK"
        env \
            HOME="$HOME_DIR" \
            GROK_OPENAI_EXPECTED_ORIGIN_URL="$ORIGIN" \
            GROK_OPENAI_EXPECTED_UPSTREAM_URL="$UPSTREAM" \
            GROK_OPENAI_GATE_CMD=true \
            GROK_OPENAI_PREBUILT="$FAKE_BINARY" \
            ./scripts/update-from-upstream.sh
    ) >"$FIXTURE/bypass.out" 2>&1; then
        fail 'updater bypass worked without explicit workflow test mode'
    fi
    grep -q 'test-only workflow overrides require GROK_OPENAI_WORKFLOW_TEST_MODE=1' "$FIXTURE/bypass.out" || fail 'updater bypass refusal was not actionable'
}

test_update_success_and_noop() {
    make_fixture update-success
    # A fresh fork clone only has origin. The updater must add the exact,
    # expected upstream fetch remote and make it push-disabled.
    git -C "$WORK" remote remove upstream
    printf '%s\n' upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-update >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null
    _test_upstream_commit=$(git -C "$UPSTREAM_WORK" rev-parse HEAD)

    _test_normal_pair=$(git -C "$WORK" show HEAD:.grok-openai-upstream)..$_test_upstream_commit
    if run_updater_accept_pair "$FIXTURE/normal-pin-refusal.out" "$_test_normal_pair"; then
        fail 'normal update unexpectedly accepted a rewrite pin'
    fi
    grep -q 'valid only when upstream no longer descends' "$FIXTURE/normal-pin-refusal.out" || \
        fail 'normal update rewrite-pin refusal was not actionable'

    GROK_OPENAI_GATE_CMD_TEST='test -f upstream.txt'
    export GROK_OPENAI_GATE_CMD_TEST
    run_updater "$FIXTURE/update.out"
    unset GROK_OPENAI_GATE_CMD_TEST

    assert_file "$WORK/upstream.txt"
    assert_file "$HOME_DIR/.local/bin/grok-openai"
    assert_absent "$HOME_DIR/.grok"
    assert_eq "$(git -C "$WORK" rev-parse HEAD)" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_eq "$(git -C "$WORK" remote get-url --push upstream)" DISABLED
    assert_eq "$(sed -n '1p' "$WORK/.grok-openai-upstream")" "$_test_upstream_commit"
    git -C "$WORK" show --format= --name-only HEAD | grep -q '^\.grok-openai-upstream$' || \
        fail 'successful merge commit did not update the upstream marker'

    _test_before=$(git -C "$WORK" rev-parse HEAD)
    run_updater "$FIXTURE/noop.out"
    assert_eq "$_test_before" "$(git -C "$WORK" rev-parse HEAD)"
    grep -q 'Already up to date' "$FIXTURE/noop.out" || fail 'no-op update was not reported'
}

test_update_accepts_unrelated_rewritten_upstream() {
    make_fixture update-unrelated-success
    _test_fork_before=$(git -C "$WORK" rev-parse HEAD)
    _test_previous_upstream=$(sed -n '1p' "$WORK/.grok-openai-upstream")

    printf '%s\n' rewritten >"$UPSTREAM_WORK/rewritten.txt"
    git -C "$UPSTREAM_WORK" add rewritten.txt
    force_rewrite_upstream
    _test_rewritten_upstream=$REWRITTEN_UPSTREAM_COMMIT

    if run_updater "$FIXTURE/unrelated-refusal.out"; then
        fail 'rewritten upstream succeeded without explicit acceptance'
    fi
    assert_eq "$_test_fork_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q -- '--accept-upstream-rewrite' "$FIXTURE/unrelated-refusal.out" || \
        fail 'rewritten upstream refusal did not explain explicit acceptance'
    if run_updater_accept_pair "$FIXTURE/unrelated-stale-pin.out" \
        "$_test_previous_upstream..$_test_previous_upstream"; then
        fail 'rewritten upstream unexpectedly accepted a stale pin'
    fi
    grep -Fq -- "--accept-upstream-rewrite=$_test_previous_upstream..$_test_rewritten_upstream" \
        "$FIXTURE/unrelated-stale-pin.out" || fail 'stale rewrite pin refusal omitted the exact current pair'

    GROK_OPENAI_GATE_CMD_TEST="test -f rewritten.txt && grep -qx $_test_rewritten_upstream .grok-openai-upstream"
    export GROK_OPENAI_GATE_CMD_TEST
    run_updater_accept_rewrite "$FIXTURE/unrelated.out"
    unset GROK_OPENAI_GATE_CMD_TEST

    assert_file "$WORK/rewritten.txt"
    assert_eq "$(sed -n '1p' "$WORK/.grok-openai-upstream")" "$_test_rewritten_upstream"
    git -C "$WORK" merge-base --is-ancestor "$_test_fork_before" HEAD || \
        fail 'unrelated update rewrote fork history instead of appending'
    git -C "$WORK" merge-base --is-ancestor "$_test_rewritten_upstream" HEAD || \
        fail 'rewritten upstream is not an ancestor of the successful merge'

    _test_bridge=$(git -C "$WORK" rev-parse HEAD^2)
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^1")" "$_test_previous_upstream"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^2")" "$_test_rewritten_upstream"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^{tree}")" \
        "$(git -C "$WORK" rev-parse "$_test_rewritten_upstream^{tree}")"
    grep -q 'synthesizing an append-only bridge' "$FIXTURE/unrelated.out" || \
        fail 'unrelated-history bridge was not reported'
}

test_update_bridges_same_root_rebase() {
    make_fixture update-same-root-rebase
    printf '%s\n' old-upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m old-upstream >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null
    run_updater "$FIXTURE/integrate-old.out"
    _test_previous_upstream=$(sed -n '1p' "$WORK/.grok-openai-upstream")

    printf '%s\n' rebased-upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    rewrite_upstream_from_parent "$UPSTREAM_BASE"
    _test_rebased_upstream=$REWRITTEN_UPSTREAM_COMMIT
    _test_fork_before=$(git -C "$WORK" rev-parse HEAD)

    if run_updater "$FIXTURE/rebase-refusal.out"; then
        fail 'same-root rebase succeeded without explicit acceptance'
    fi
    assert_eq "$_test_fork_before" "$(git -C "$WORK" rev-parse HEAD)"
    grep -q -- '--accept-upstream-rewrite' "$FIXTURE/rebase-refusal.out" || \
        fail 'same-root rebase refusal did not explain explicit acceptance'

    run_updater_accept_rewrite "$FIXTURE/rebase.out"
    _test_bridge=$(git -C "$WORK" rev-parse HEAD^2)
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^1")" "$_test_previous_upstream"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^2")" "$_test_rebased_upstream"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^{tree}")" \
        "$(git -C "$WORK" rev-parse "$_test_rebased_upstream^{tree}")"
    assert_eq "$(sed -n '1p' "$WORK/.grok-openai-upstream")" "$_test_rebased_upstream"
}

test_update_requires_acceptance_for_rollback() {
    make_fixture update-rollback
    printf '%s\n' upstream >"$UPSTREAM_WORK/rollback.txt"
    git -C "$UPSTREAM_WORK" add rollback.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-before-rollback >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null
    run_updater "$FIXTURE/integrate-before-rollback.out"
    _test_previous_upstream=$(sed -n '1p' "$WORK/.grok-openai-upstream")
    _test_fork_before=$(git -C "$WORK" rev-parse HEAD)

    git -C "$UPSTREAM_WORK" push --force origin \
        "$UPSTREAM_BASE:refs/heads/main" >/dev/null
    if run_updater "$FIXTURE/rollback-refusal.out"; then
        fail 'upstream rollback succeeded without explicit acceptance'
    fi
    assert_eq "$_test_fork_before" "$(git -C "$WORK" rev-parse HEAD)"
    grep -q -- '--accept-upstream-rewrite' "$FIXTURE/rollback-refusal.out" || \
        fail 'rollback refusal did not explain explicit acceptance'

    GROK_OPENAI_GATE_CMD_TEST='test ! -e rollback.txt'
    export GROK_OPENAI_GATE_CMD_TEST
    run_updater_accept_rewrite "$FIXTURE/rollback.out"
    unset GROK_OPENAI_GATE_CMD_TEST
    assert_absent "$WORK/rollback.txt"
    _test_bridge=$(git -C "$WORK" rev-parse HEAD^2)
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^1")" "$_test_previous_upstream"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^2")" "$UPSTREAM_BASE"
    assert_eq "$(git -C "$WORK" rev-parse "$_test_bridge^{tree}")" \
        "$(git -C "$WORK" rev-parse "$UPSTREAM_BASE^{tree}")"
    assert_eq "$(sed -n '1p' "$WORK/.grok-openai-upstream")" "$UPSTREAM_BASE"
}

test_update_rejects_malformed_marker() {
    make_fixture update-malformed-marker
    printf '%s\n' 'NOT-A-FULL-LOWERCASE-SHA' >"$WORK/.grok-openai-upstream"
    git -C "$WORK" add .grok-openai-upstream
    git -C "$WORK" commit -m 'malformed marker fixture' >/dev/null
    git -C "$WORK" push origin main >/dev/null
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)

    if run_updater "$FIXTURE/malformed-marker.out"; then
        fail 'malformed upstream marker unexpectedly succeeded'
    fi
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'must contain exactly one full lowercase commit SHA' "$FIXTURE/malformed-marker.out" || \
        fail 'malformed marker refusal was not actionable'
}

test_update_rejects_nonancestor_marker() {
    make_fixture update-nonancestor-marker
    printf '%s\n' upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-update >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null
    _test_nonancestor=$(git -C "$UPSTREAM_WORK" rev-parse HEAD)

    printf '%s\n' "$_test_nonancestor" >"$WORK/.grok-openai-upstream"
    git -C "$WORK" add .grok-openai-upstream
    git -C "$WORK" commit -m 'nonancestor marker fixture' >/dev/null
    git -C "$WORK" push origin main >/dev/null
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)

    if run_updater "$FIXTURE/nonancestor-marker.out"; then
        fail 'nonancestor upstream marker unexpectedly succeeded'
    fi
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'commit is not an ancestor of fork HEAD' "$FIXTURE/nonancestor-marker.out" || \
        fail 'nonancestor marker refusal was not actionable'
}

test_update_rejects_dirty_main() {
    make_fixture update-dirty
    printf '%s\n' dirty >"$WORK/untracked.txt"
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    if run_updater "$FIXTURE/dirty.out"; then
        fail 'dirty update unexpectedly succeeded'
    fi
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'must be completely clean' "$FIXTURE/dirty.out" || fail 'dirty failure was not actionable'
}

test_update_preserves_state_on_conflict() {
    make_fixture update-conflict
    printf '%s\n' fork >"$WORK/conflict.txt"
    git -C "$WORK" add conflict.txt
    git -C "$WORK" commit -m fork-conflict >/dev/null
    git -C "$WORK" push origin main >/dev/null

    printf '%s\n' upstream >"$UPSTREAM_WORK/conflict.txt"
    git -C "$UPSTREAM_WORK" add conflict.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-conflict >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null

    _test_local_before=$(git -C "$WORK" rev-parse HEAD)
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    if run_updater "$FIXTURE/conflict.out"; then
        fail 'conflicting update unexpectedly succeeded'
    fi
    assert_eq "$_test_local_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'Candidate retained for inspection' "$FIXTURE/conflict.out" || fail 'conflict recovery path was not reported'
}

test_unrelated_update_preserves_state_on_conflict() {
    make_fixture update-unrelated-conflict
    _test_previous_upstream=$(sed -n '1p' "$WORK/.grok-openai-upstream")
    printf '%s\n' fork >"$WORK/conflict.txt"
    git -C "$WORK" add conflict.txt
    git -C "$WORK" commit -m fork-conflict >/dev/null
    git -C "$WORK" push origin main >/dev/null

    printf '%s\n' rewritten-upstream >"$UPSTREAM_WORK/conflict.txt"
    git -C "$UPSTREAM_WORK" add conflict.txt
    force_rewrite_upstream

    _test_local_before=$(git -C "$WORK" rev-parse HEAD)
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    if run_updater_accept_rewrite "$FIXTURE/unrelated-conflict.out"; then
        fail 'conflicting unrelated update unexpectedly succeeded'
    fi
    assert_eq "$_test_local_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_eq "$_test_previous_upstream" "$(sed -n '1p' "$WORK/.grok-openai-upstream")"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'synthesizing an append-only bridge' "$FIXTURE/unrelated-conflict.out" || \
        fail 'unrelated conflict did not enter the bridge path'
    grep -q 'Candidate retained for inspection' "$FIXTURE/unrelated-conflict.out" || \
        fail 'unrelated conflict candidate was not retained'
}

test_update_detects_upstream_race() {
    make_fixture update-upstream-race
    printf '%s\n' first >"$UPSTREAM_WORK/race.txt"
    git -C "$UPSTREAM_WORK" add race.txt
    git -C "$UPSTREAM_WORK" commit -m first-upstream >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null

    printf '%s\n' second >"$UPSTREAM_WORK/race.txt"
    git -C "$UPSTREAM_WORK" add race.txt
    git -C "$UPSTREAM_WORK" commit -m second-upstream >/dev/null
    _test_second_upstream=$(git -C "$UPSTREAM_WORK" rev-parse HEAD)
    git -C "$UPSTREAM_WORK" push origin HEAD:refs/heads/race-next >/dev/null

    _test_local_before=$(git -C "$WORK" rev-parse HEAD)
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    GROK_OPENAI_GATE_CMD_TEST="git --git-dir=$UPSTREAM update-ref refs/heads/main $_test_second_upstream"
    export GROK_OPENAI_GATE_CMD_TEST
    if run_updater "$FIXTURE/upstream-race.out"; then
        unset GROK_OPENAI_GATE_CMD_TEST
        fail 'upstream race unexpectedly published the stale candidate'
    fi
    unset GROK_OPENAI_GATE_CMD_TEST
    assert_eq "$_test_local_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'upstream/main changed during validation' "$FIXTURE/upstream-race.out" || \
        fail 'upstream race refusal was not actionable'
    grep -q 'Candidate retained for inspection' "$FIXTURE/upstream-race.out" || \
        fail 'validated stale candidate was not retained for inspection'
}

test_candidate_gate_sanitizes_ambient_credentials() {
    make_fixture update-sanitized-gate
    printf '%s\n' upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-update >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null

    _test_sanitized_gate=$FIXTURE/sanitized-gate.sh
    cat >"$_test_sanitized_gate" <<'EOF'
#!/bin/sh
case ${HOME:-} in
    */validation-home) ;;
    *) exit 1 ;;
esac
if env | grep -Eq '^(OPENAI_API_KEY|XAI_API_KEY|SUPABASE_MCP_BEARER_TOKEN|CANDIDATE_SECRET_SENTINEL)='; then
    exit 1
fi
EOF
    chmod 755 "$_test_sanitized_gate"
    GROK_OPENAI_GATE_CMD_TEST=$_test_sanitized_gate
    export GROK_OPENAI_GATE_CMD_TEST
    run_updater "$FIXTURE/sanitized-gate.out" \
        OPENAI_API_KEY=must-not-reach-candidate \
        XAI_API_KEY=must-not-reach-candidate \
        SUPABASE_MCP_BEARER_TOKEN=must-not-reach-candidate \
        CANDIDATE_SECRET_SENTINEL=must-not-reach-candidate
    unset GROK_OPENAI_GATE_CMD_TEST
    assert_file "$HOME_DIR/.local/bin/grok-openai"
}

test_update_rejects_gate_mutated_commit() {
    make_fixture update-mutated-candidate
    printf '%s\n' upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-update >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null

    _test_local_before=$(git -C "$WORK" rev-parse HEAD)
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    GROK_OPENAI_GATE_CMD_TEST='git -c commit.gpgSign=false -c core.hooksPath=/dev/null commit --allow-empty --no-verify -m gate-mutated >/dev/null'
    export GROK_OPENAI_GATE_CMD_TEST
    if run_updater "$FIXTURE/mutated-candidate.out"; then
        unset GROK_OPENAI_GATE_CMD_TEST
        fail 'gate-mutated candidate commit unexpectedly published'
    fi
    unset GROK_OPENAI_GATE_CMD_TEST
    assert_eq "$_test_local_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'candidate HEAD changed during validation' "$FIXTURE/mutated-candidate.out" || \
        fail 'gate-mutated candidate refusal was not actionable'
    grep -q 'Candidate retained for inspection' "$FIXTURE/mutated-candidate.out" || \
        fail 'gate-mutated candidate was not retained'
}

test_update_preserves_state_on_gate_failure() {
    make_fixture update-gate-failure
    printf '%s\n' upstream >"$UPSTREAM_WORK/upstream.txt"
    git -C "$UPSTREAM_WORK" add upstream.txt
    git -C "$UPSTREAM_WORK" commit -m upstream-update >/dev/null
    git -C "$UPSTREAM_WORK" push origin main >/dev/null

    _test_local_before=$(git -C "$WORK" rev-parse HEAD)
    _test_origin_before=$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)
    GROK_OPENAI_GATE_CMD_TEST=false
    export GROK_OPENAI_GATE_CMD_TEST
    if run_updater "$FIXTURE/gate-failure.out"; then
        unset GROK_OPENAI_GATE_CMD_TEST
        fail 'failed candidate gate unexpectedly succeeded'
    fi
    unset GROK_OPENAI_GATE_CMD_TEST
    assert_eq "$_test_local_before" "$(git -C "$WORK" rev-parse HEAD)"
    assert_eq "$_test_origin_before" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_absent "$HOME_DIR/.local/bin/grok-openai"
    grep -q 'candidate validation gate failed' "$FIXTURE/gate-failure.out" || fail 'gate failure was not actionable'
}

test_install_isolation
test_bypasses_require_explicit_test_mode
test_update_success_and_noop
test_update_accepts_unrelated_rewritten_upstream
test_update_bridges_same_root_rebase
test_update_requires_acceptance_for_rollback
test_update_rejects_malformed_marker
test_update_rejects_nonancestor_marker
test_update_rejects_dirty_main
test_update_preserves_state_on_conflict
test_unrelated_update_preserves_state_on_conflict
test_update_detects_upstream_race
test_candidate_gate_sanitizes_ambient_credentials
test_update_rejects_gate_mutated_commit
test_update_preserves_state_on_gate_failure

printf '%s\n' 'OpenAI shell workflow tests passed.'
