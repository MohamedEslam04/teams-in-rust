#!/usr/bin/env bash
# Records the size of a built release binary and the number of third-party dependencies of
# `ironteams`, with and without the `mcp` feature, in the job summary, so the pull request that
# sets the repository variable IRONTEAMS_MCP can cite before and after (the size of the two builds
# comes from one release run each, the dependency counts from either).
#
#   bash scripts/ci/release-summary.sh <binary> <label>
#
# Informational: every failure here is a warning, never a failed release. Release profile builds
# are stripped already (`strip = true` in Cargo.toml), so the size is that of the shipped binary.
set -uo pipefail

bin=${1:?usage: release-summary.sh <binary> <label>}
label=${2:?usage: release-summary.sh <binary> <label>}
out=${GITHUB_STEP_SUMMARY:-/dev/stdout}

# The unique packages of the normal dependency tree (`(*)` marks a repeat of a line above).
count() {
    cargo tree -p ironteams -e normal --prefix none --locked "$@" | sed 's/ (\*)$//' | sort -u | wc -l | tr -d ' '
}

if [[ ! -f "$bin" ]]; then
    echo "::warning::release summary: $bin not found"
    exit 0
fi
# `wc -c < file` is the same on Linux, macOS and Git Bash (`stat` differs).
size=$(wc -c < "$bin" | tr -d ' ')
without=$(count) || without='?'
with=$(count --features mcp) || with='?'
mcp=off
[[ "${IRONTEAMS_MCP:-}" == 1 ]] && mcp=on

{
    echo "### Release binary: $label"
    echo
    echo "| | |"
    echo "|---|---|"
    echo "| MCP server (variable IRONTEAMS_MCP) | $mcp |"
    echo "| Stripped binary | $size bytes ($((size / 1024)) KiB) |"
    echo "| Dependencies without the feature | $without |"
    echo "| Dependencies with the feature | $with |"
    echo
} >> "$out"
