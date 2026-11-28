#!/usr/bin/env bash
# The release build of the shipped binary ($BIN), shared by release.yml, release-add.yml and
# cache-warm.yml so they stay the same build. It is `cargo build --release --locked -p "$BIN"
# --bin "$BIN"`, plus `--features mcp` when the repository variable IRONTEAMS_MCP is 1 (passed in
# as the environment variable of that name), the binary is `ironteams` (the only package with that
# feature, docs/designs/mcp-access-control.md) and its manifest has the feature (an older commit
# does not). With the variable unset or anything else, the command is exactly the one the
# workflows ran before. (release-add.yml builds the tag's own commit, which may predate this file,
# so it has the same logic inline.)
set -euo pipefail

features=()
if [[ "${IRONTEAMS_MCP:-}" == 1 && "$BIN" == ironteams ]] && grep -q '^mcp = ' "crates/$BIN/Cargo.toml"; then
    features=(--features mcp)
    echo "IRONTEAMS_MCP=1: building $BIN with the MCP server (--features mcp)"
fi
# `${a[@]+...}`: an empty array is not an unset-variable error on the macOS bash (3.2).
exec cargo build --release --locked -p "$BIN" --bin "$BIN" ${features[@]+"${features[@]}"}
