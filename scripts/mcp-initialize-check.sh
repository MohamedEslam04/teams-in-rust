#!/usr/bin/env bash
# Proves that `ironteams --mcp-serve` runs as a flag mode with piped stdin/stdout/stderr, the way
# an MCP client starts it: no window, no hang, and a clear error on stderr. CI has no signed-in
# account and no keyring, so it runs the server for an account that does not exist, in an empty
# IRONTEAMS_HOME. The server must then
#   - exit by itself within 20 seconds, with a non-zero status,
#   - say `no account "x"` on stderr,
#   - print nothing on stdout (stdout is the MCP channel).
# A real `initialize` round trip (a JSON-RPC response whose result.serverInfo.name is "ironteams")
# needs a signed-in account, so it is part of the live test (PLAN.md, "Needs the user"):
#   printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ci","version":"0"}}}' \
#     | ironteams --mcp-serve --account <label>
# The request is sent here too, so that this check also covers a server that reads stdin first.
#
#   bash scripts/mcp-initialize-check.sh <path to a binary built with --features mcp>
# Windows: scripts/mcp-initialize-check.ps1 (the release binary there is a GUI-subsystem program).
set -uo pipefail

exe=${1:?usage: mcp-initialize-check.sh <ironteams binary>}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
request='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ci","version":"0"}}}'

printf '%s\n' "$request" | IRONTEAMS_HOME="$work/home" "$exe" --mcp-serve --account x >"$work/out" 2>"$work/err" &
pid=$!
# No `timeout` on macOS: poll for 20 seconds.
for _ in $(seq 1 200); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" 2>/dev/null
    echo "FAIL: $exe --mcp-serve did not exit within 20 s"
    exit 1
fi
wait "$pid"
code=$?

fail=0
if [ "$code" -eq 0 ]; then
    echo "FAIL: exit status 0 for an account that does not exist"
    fail=1
fi
if [ -s "$work/out" ]; then
    echo "FAIL: stdout is not empty (it is the MCP channel):"
    cat "$work/out"
    fail=1
fi
if ! grep -q 'no account "x"' "$work/err"; then
    echo "FAIL: stderr does not say 'no account \"x\"':"
    cat "$work/err"
    fail=1
fi
if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "ok: --mcp-serve exited with status $code and the expected error:"
cat "$work/err"
