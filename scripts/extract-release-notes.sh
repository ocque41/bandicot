#!/bin/sh
set -eu

VERSION=${1:?usage: extract-release-notes.sh VERSION [CHANGELOG]}
CHANGELOG=${2:-CHANGELOG.md}

case $VERSION in
    *[!0-9A-Za-z.+-]*|'')
        printf 'error: invalid release version: %s\n' "$VERSION" >&2
        exit 2
        ;;
esac

awk -v version="$VERSION" '
    $0 ~ "^## \\[" version "\\]" { capture=1; print; next }
    capture && /^## \[/ { exit }
    capture { print }
    END { if (!capture) exit 1 }
' "$CHANGELOG"
