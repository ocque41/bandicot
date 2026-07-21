#!/bin/sh
set -eu
umask 077

KEYCHAIN_SERVICE=${BANDICOT_CEREBRAS_KEYCHAIN_SERVICE:-bandicot.cerebras}
KEYCHAIN_ACCOUNT=${BANDICOT_CEREBRAS_KEYCHAIN_ACCOUNT:-cerebras-cloud}
REPLACE=0

case ${1:-} in
    '') ;;
    --replace) REPLACE=1 ;;
    -h|--help)
        cat <<'EOF'
Usage: scripts/setup-cerebras-key.sh [--replace]

Store a Cerebras Cloud API key through the macOS Keychain prompt. Existing
credentials are preserved unless --replace is supplied.
EOF
        exit 0
        ;;
    *)
        printf 'error: unknown argument: %s\n' "$1" >&2
        exit 1
        ;;
esac

if [ "$REPLACE" -eq 0 ] && [ -n "${CEREBRAS_API_KEY:-}" ]; then
    printf '%s\n' 'CEREBRAS_API_KEY is already present in this process environment.'
    printf '%s\n' 'No key was read, printed, or written.'
    exit 0
fi

if [ ! -x /usr/bin/security ]; then
    printf '%s\n' 'error: macOS Keychain is unavailable at /usr/bin/security.' >&2
    printf '%s\n' 'Set CEREBRAS_API_KEY in the process that launches Bandicot instead.' >&2
    exit 1
fi

if [ "$REPLACE" -eq 0 ] && /usr/bin/security find-generic-password \
    -a "$KEYCHAIN_ACCOUNT" \
    -s "$KEYCHAIN_SERVICE" \
    -w >/dev/null 2>&1; then
    printf '%s\n' 'A Cerebras API key is already stored in macOS Keychain for Bandicot.'
    printf '%s\n' 'Run scripts/setup-cerebras-key.sh --replace to replace it securely.'
    exit 0
fi

printf '%s\n' 'macOS Keychain will now prompt for your Cerebras Cloud API key.'
printf '%s\n' 'The key is passed directly to Keychain; this script does not echo or save it in a file.'

/usr/bin/security add-generic-password \
    -U \
    -a "$KEYCHAIN_ACCOUNT" \
    -s "$KEYCHAIN_SERVICE" \
    -l 'Bandicot Cerebras Cloud API key' \
    -w

if ! /usr/bin/security find-generic-password \
    -a "$KEYCHAIN_ACCOUNT" \
    -s "$KEYCHAIN_SERVICE" \
    -w >/dev/null 2>&1; then
    printf '%s\n' 'error: Keychain did not return the newly stored credential' >&2
    exit 1
fi

printf '%s\n' 'Cerebras API key stored in macOS Keychain for Bandicot.'
