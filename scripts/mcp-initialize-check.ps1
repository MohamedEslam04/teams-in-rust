# Proves that the Windows release binary (a GUI-subsystem program) runs `--mcp-serve` as a flag
# mode with piped stdin/stdout/stderr, the way an MCP client starts it: no window, no hang, and a
# clear error on stderr. CI has no signed-in account and no keyring, so it runs the server for an
# account that does not exist, in an empty IRONTEAMS_HOME. The server must then
#   - exit by itself within 20 seconds, with a non-zero status,
#   - say `no account "x"` on stderr,
#   - print nothing on stdout (stdout is the MCP channel).
# A real `initialize` round trip (a JSON-RPC response whose result.serverInfo.name is "ironteams")
# needs a signed-in account, so it is part of the live test (PLAN.md, "Needs the user"). The
# request is sent here too, so that this check also covers a server that reads stdin first.
#
#   pwsh scripts/mcp-initialize-check.ps1 -Exe target\release\ironteams.exe
# (scripts/mcp-initialize-check.sh does the same on Linux and macOS.)
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = 'Stop'

$request = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ci","version":"0"}}}'
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("ironteams-mcp-check-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = (Resolve-Path $Exe).Path
$psi.Arguments = '--mcp-serve --account x'
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables['IRONTEAMS_HOME'] = Join-Path $work 'home'

$p = [System.Diagnostics.Process]::Start($psi)
# Read both pipes while it runs, so a full pipe cannot block it.
$out = $p.StandardOutput.ReadToEndAsync()
$err = $p.StandardError.ReadToEndAsync()
try {
    $p.StandardInput.WriteLine($request)
    $p.StandardInput.Close()
} catch {
    # The server may already have exited: a closed pipe is fine.
}
if (-not $p.WaitForExit(20000)) {
    $p.Kill()
    Write-Host "FAIL: $Exe --mcp-serve did not exit within 20 s"
    exit 1
}
$p.WaitForExit()   # lets the asynchronous reads finish
$stdout = $out.Result
$stderr = $err.Result
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue

$fail = $false
if ($p.ExitCode -eq 0) {
    Write-Host 'FAIL: exit status 0 for an account that does not exist'
    $fail = $true
}
if ($stdout.Trim().Length -ne 0) {
    Write-Host "FAIL: stdout is not empty (it is the MCP channel): $stdout"
    $fail = $true
}
if (-not $stderr.Contains('no account "x"')) {
    Write-Host "FAIL: stderr does not say 'no account ""x""': $stderr"
    $fail = $true
}
if ($fail) { exit 1 }
Write-Host "ok: --mcp-serve exited with status $($p.ExitCode) and the expected error: $stderr"
