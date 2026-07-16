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
    assert_file "$_test_home/.local/bin/grok-openai"
    assert_file "$_test_home/.grok-openai/config.toml"
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
    printf '%s\n' base >"$SEED/conflict.txt"
    printf '%s\n' base >"$SEED/base.txt"
    git -C "$SEED" add .
    git -C "$SEED" commit -m base >/dev/null
    git -C "$SEED" remote add origin "$ORIGIN"
    git -C "$SEED" remote add upstream "$UPSTREAM"
    git -C "$SEED" push origin main >/dev/null
    git -C "$SEED" push upstream main >/dev/null
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

    GROK_OPENAI_GATE_CMD_TEST='test -f upstream.txt'
    export GROK_OPENAI_GATE_CMD_TEST
    run_updater "$FIXTURE/update.out"
    unset GROK_OPENAI_GATE_CMD_TEST

    assert_file "$WORK/upstream.txt"
    assert_file "$HOME_DIR/.local/bin/grok-openai"
    assert_absent "$HOME_DIR/.grok"
    assert_eq "$(git -C "$WORK" rev-parse HEAD)" "$(git --git-dir="$ORIGIN" rev-parse refs/heads/main)"
    assert_eq "$(git -C "$WORK" remote get-url --push upstream)" DISABLED

    _test_before=$(git -C "$WORK" rev-parse HEAD)
    run_updater "$FIXTURE/noop.out"
    assert_eq "$_test_before" "$(git -C "$WORK" rev-parse HEAD)"
    grep -q 'Already up to date' "$FIXTURE/noop.out" || fail 'no-op update was not reported'
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
test_update_rejects_dirty_main
test_update_preserves_state_on_conflict
test_update_preserves_state_on_gate_failure

printf '%s\n' 'OpenAI shell workflow tests passed.'
