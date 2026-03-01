#!/bin/sh
set -eu

if [ $# -ne 1 ]; then
    printf 'Usage: %s <new-version>\n' "$0" >&2
    exit 1
fi

VERSION=$1

# Strip leading 'v' if provided
VERSION=${VERSION#v}

# Update Cargo.toml
sd 'version = "[^"]*"' "version = \"$VERSION\"" Cargo.toml

# Update Cargo.lock by running a no-op cargo command
cargo update --workspace 2>/dev/null || true

git add Cargo.toml Cargo.lock
git commit -m "v$VERSION"
git tag "v$VERSION"
git push
git push --tags

printf 'Bumped to v%s and pushed tag\n' "$VERSION"
