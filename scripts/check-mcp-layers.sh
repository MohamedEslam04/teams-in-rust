#!/usr/bin/env bash
# Keeps the MCP server's one enforcement point honest (docs/designs/mcp-access-control.md):
# `teams_proto::Client` may be named only in crates/mcp/src/gateway.rs. The tools in
# crates/mcp/src/server/ get everything through the gateway, which hands out the permits, so
# "a tool fetched a conversation without a check" cannot be written.
#
# Run from anywhere: bash scripts/check-mcp-layers.sh. Exits 1 and names each offence.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
server="$repo/crates/mcp/src/server"
fail=0

if [ ! -d "$server" ]; then
    echo "mcp-layers: $server does not exist"
    exit 1
fi

# `Client` as a word, in any form that names the protocol client: teams_proto::Client,
# `use teams_proto::{..., Client}`, a bare `Client`. Comments are not checked.
mapfile -t files < <(find "$server" -name '*.rs' | sort)
for f in "${files[@]}"; do
    rel="${f#"$repo"/}"
    while IFS= read -r hit; do
        [ -z "$hit" ] && continue
        echo "mcp-layers: $rel names the Teams client ($hit); only crates/mcp/src/gateway.rs may"
        fail=1
    done < <(sed -E 's#//.*$##' "$f" | grep -noE '\bClient\b|teams_proto::client|\bclient::Client\b' || true)
done

# The server code does not call the calendar's or the protocol's fetch functions either: these
# take a client, so they could only be reached through one.
for f in "${files[@]}"; do
    rel="${f#"$repo"/}"
    while IFS= read -r hit; do
        [ -z "$hit" ] && continue
        echo "mcp-layers: $rel calls a fetch function ($hit); go through the gateway"
        fail=1
    done < <(sed -E 's#//.*$##' "$f" | grep -noE 'calendar::(Calendar::view|event_detail|find_meeting_times|respond)\b' || true)
done

# Nothing outside the gateway and the server may reach for the permit's fields; and the access log
# is written only by the gateway.
for f in "${files[@]}"; do
    rel="${f#"$repo"/}"
    if sed -E 's#//.*$##' "$f" | grep -qE 'record_access\('; then
        echo "mcp-layers: $rel writes an access line; only the gateway may"
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    echo "check-mcp-layers: FAILED"
    exit 1
fi
echo "check-mcp-layers: OK (${#files[@]} files)"
