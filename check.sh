#!/bin/sh
# The single gate every change must pass before it is handed back.
#
# Run from anywhere: it resolves the workspace from its own location, and picks
# up the toolchain from ~/.cargo, which rustup installed with --no-modify-path
# and is therefore not on PATH by default.
set -eu

WORKSPACE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Safe under `set -e` even when the file is absent: POSIX exempts a failing
# command that is not the last in an AND-OR list, so a machine without rustup
# skips the sourcing and carries on rather than aborting before the first check.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

cd "$WORKSPACE"

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy (warnings are errors)"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test"
cargo test --workspace

echo "OK"
