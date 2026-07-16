#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd -P)

# An explicit override exists only for the repository's hermetic workflow
# tests. Refuse it unless test mode is equally explicit, so an ambient variable
# can never skip production validation.
if [ -n "${GROK_OPENAI_GATE_CMD:-}" ]; then
    [ "${GROK_OPENAI_WORKFLOW_TEST_MODE:-0}" = 1 ] || {
        printf '%s\n' 'error: GROK_OPENAI_GATE_CMD requires GROK_OPENAI_WORKFLOW_TEST_MODE=1' >&2
        exit 1
    }
    openai_gate_cmd=$GROK_OPENAI_GATE_CMD
    unset GROK_OPENAI_GATE_CMD
    cd "$REPO_ROOT"
    exec /bin/sh -c "$openai_gate_cmd"
fi

cd "$REPO_ROOT"

# Pin the artifact location explicitly so global Cargo configuration cannot
# make the caller install a different path than the one validated here.
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
case $CARGO_TARGET_DIR in
    /*) ;;
    *) CARGO_TARGET_DIR=$REPO_ROOT/$CARGO_TARGET_DIR ;;
esac
export CARGO_TARGET_DIR
# This workspace produces very large, feature-specific incremental caches and
# the validator already does a clean semantic pass. Keep the one-command update
# bounded in disk usage instead of retaining disposable compiler state.
export CARGO_INCREMENTAL=0

command -v cargo >/dev/null 2>&1 || {
    printf '%s\n' 'error: cargo is required to validate this source build' >&2
    exit 1
}

RG_PATH=${GROK_SHELL_BUNDLE_RG_PATH:-}
if [ -z "$RG_PATH" ]; then
    RG_PATH=$(command -v rg 2>/dev/null || true)
fi
if [ -z "$RG_PATH" ] || [ ! -x "$RG_PATH" ]; then
    printf '%s\n' 'error: an executable ripgrep (rg) is required for the offline-safe release build' >&2
    printf '%s\n' 'Set GROK_SHELL_BUNDLE_RG_PATH to the rg binary and retry.' >&2
    exit 1
fi

printf '%s\n' 'Checking shell workflows...'
/bin/sh -n \
    scripts/install-openai.sh \
    scripts/setup-openai-key.sh \
    scripts/update-from-upstream.sh \
    scripts/validate-openai.sh \
    scripts/tests/run.sh
scripts/tests/run.sh

printf '%s\n' 'Checking Rust formatting...'
cargo fmt --all -- --check

printf '%s\n' 'Checking OpenAI provider and application crates...'
cargo check --locked \
    -p xai-grok-models \
    -p xai-grok-sampling-types \
    -p xai-grok-sampler \
    -p xai-grok-shell \
    -p xai-grok-pager-bin

printf '%s\n' 'Running OpenAI provider, credential, stream, and profile tests...'
cargo test --locked -p xai-grok-models
cargo test --locked -p xai-grok-sampling-types
cargo test --locked -p xai-grok-sampler --lib
cargo test --locked -p xai-grok-sampler --test provider_wire
# The shell's production dependency surface is compiled by `cargo check` above.
# Its monolithic `--lib` test target cannot be used as a release gate because
# several upstream dependencies intentionally hide their test helpers behind
# dependency-local `cfg(test)`. Exercise the provider path through integration
# targets instead, which compile the shell exactly as the shipped binary does.
cargo test --locked -p xai-grok-shell-base --test provider_boundary
cargo test --locked -p xai-grok-shell --test test_sampling_client test_responses_api_streaming_text
cargo test --locked -p xai-grok-shell --test test_sampling_client test_responses_api_streaming_tool_call
cargo test --locked -p xai-grok-shell --test test_sampling_client test_responses_api_multi_turn_with_tool_calls
cargo test --locked -p xai-grok-shell --test openai_distribution_acceptance
# Like the shell, the pager's monolithic unit-test target imports helpers that
# dependency crates keep behind dependency-local `cfg(test)`. This integration
# target compiles the pager exactly as shipped and exercises the public bridge.
cargo test --locked -p xai-grok-pager --test voice_auth_boundary

printf '%s\n' 'Building the hardened release-dist pager binary...'
GROK_SHELL_BUNDLE_RG_PATH=$RG_PATH \
    cargo build --locked \
        -p xai-grok-pager-bin \
        --bin xai-grok-pager \
        --profile release-dist \
        --features release-dist

RELEASE_BINARY=$CARGO_TARGET_DIR/release-dist/xai-grok-pager
if [ ! -x "$RELEASE_BINARY" ]; then
    printf 'error: release build did not produce %s\n' "$RELEASE_BINARY" >&2
    exit 1
fi
"$RELEASE_BINARY" --version >/dev/null

# The launcher sets this fork-only guard. Test the actual parser/dispatch path
# with a global option preceding the subcommand, which previously bypassed the
# launcher's fast first-argument check. `--check` makes this non-mutating even
# if a future regression reaches the vendor update implementation.
VENDOR_UPDATE_TEST_OUTPUT=$CARGO_TARGET_DIR/vendor-update-refusal.out
if GROK_OPENAI_DISABLE_VENDOR_UPDATE=1 \
    "$RELEASE_BINARY" --log-sampling update --check \
        >"$VENDOR_UPDATE_TEST_OUTPUT" 2>&1; then
    printf '%s\n' 'error: release binary accepted the disabled vendor updater' >&2
    exit 1
fi
grep -q 'the vendor updater is disabled for grok-openai' "$VENDOR_UPDATE_TEST_OUTPUT" || {
    printf '%s\n' 'error: release binary vendor-update refusal was not actionable' >&2
    exit 1
}

printf '%s\n' 'Running the built binary against the authenticated local OpenAI Responses mock...'
GROK_BINARY=$RELEASE_BINARY \
    cargo test --locked \
        -p xai-grok-shell \
        --test openai_distribution_acceptance \
        built_binary_uses_checked_in_openai_profile_end_to_end \
        -- \
        --ignored \
        --exact

printf 'OpenAI validation passed. Release binary: %s\n' "$RELEASE_BINARY"
