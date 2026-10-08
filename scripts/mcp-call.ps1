<#
.SYNOPSIS
Call one Gen-Audio MCP tool over HTTP JSON-RPC (tools/call) and print the result.

.DESCRIPTION
Finds the server the same way the desktop app binds it, then checks that the
port really is Gen-Audio (initialize, serverInfo.name = gen-audio):
  1. -Port, when given
  2. GEN_AUDIO_MCP_ADDR (host:port), the desktop app's preferred address
  3. 127.0.0.1:8765, the default
  4. Windows only: loopback ports a gen-audio / gen-audio-mcp process is
     listening on (the app falls back to an ephemeral port when 8765 is taken)
Prints `MCP addr=<host:port> tool=<name>` and then the JSON-RPC result.
Exits non-zero on a JSON-RPC error or a tool result with isError true.
Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
pwsh -NoProfile -File scripts/mcp-call.ps1 -Tool ui_navigate -ArgsJson '{"slide":"slide:library"}'
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Tool,
    [string]$ArgsJson = '{}',
    [int]$Port = 0,
    [int]$TimeoutSec = 15
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

function Invoke-JsonRpc {
    param([Parameter(Mandatory = $true)][string]$Address, [Parameter(Mandatory = $true)][string]$Body, [int]$Timeout = 5)
    $response = Invoke-WebRequest -Uri "http://$Address/mcp" -Method POST -ContentType 'application/json' -Body ([System.Text.Encoding]::UTF8.GetBytes($Body)) -UseBasicParsing -TimeoutSec $Timeout
    $text = $response.Content
    if ($text -is [byte[]]) { $text = [System.Text.Encoding]::UTF8.GetString($text) }
    return $text
}

function Test-GenAudioMcp {
    [OutputType([bool])]
    param([Parameter(Mandatory = $true)][string]$Address)
    $init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"mcp-call","version":"0.1.0"}}}'
    try {
        $reply = (Invoke-JsonRpc -Address $Address -Body $init -Timeout 3) | ConvertFrom-Json
        return ($reply.result.serverInfo.name -eq 'gen-audio')
    } catch {
        Write-Verbose "no Gen-Audio MCP at ${Address}: $($_.Exception.Message)"
        return $false
    }
}

try {
    $arguments = $ArgsJson | ConvertFrom-Json
} catch {
    throw "-ArgsJson is not valid JSON: $ArgsJson"
}
if ($null -eq $arguments -or $arguments -is [array] -or $arguments -is [string] -or $arguments -is [ValueType]) {
    throw "-ArgsJson must be a JSON object, got: $ArgsJson"
}

$candidates = New-Object System.Collections.Generic.List[string]
if ($Port -gt 0) {
    $candidates.Add("127.0.0.1:$Port")
} else {
    if ($env:GEN_AUDIO_MCP_ADDR) { $candidates.Add($env:GEN_AUDIO_MCP_ADDR) }
    $candidates.Add('127.0.0.1:8765')
    if ((Test-IsWindowsHost) -and (Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue)) {
        $owners = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'gen-audio*' } | ForEach-Object { $_.Id })
        if ($owners.Count -gt 0) {
            $listening = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
                    Where-Object { $owners -contains $_.OwningProcess -and ($_.LocalAddress -eq '127.0.0.1' -or $_.LocalAddress -eq '::1') })
            foreach ($socket in $listening) {
                $address = if ($socket.LocalAddress -eq '::1') { "[::1]:$($socket.LocalPort)" } else { "127.0.0.1:$($socket.LocalPort)" }
                if (-not $candidates.Contains($address)) { $candidates.Add($address) }
            }
        }
    }
}

$addr = $null
foreach ($candidate in $candidates) {
    if (Test-GenAudioMcp -Address $candidate) { $addr = $candidate; break }
}
if (-not $addr) { throw "no Gen-Audio MCP server answered initialize on: $($candidates -join ', ')" }

$request = @{ jsonrpc = '2.0'; id = 2; method = 'tools/call'; params = @{ name = $Tool; arguments = $arguments } } | ConvertTo-Json -Depth 32 -Compress
$text = Invoke-JsonRpc -Address $addr -Body $request -Timeout $TimeoutSec
Write-Output "MCP addr=$addr tool=$Tool"
Write-Output $text
$reply = $text | ConvertFrom-Json
if ($reply.PSObject.Properties.Name -contains 'error') {
    Write-Error "JSON-RPC error $($reply.error.code): $($reply.error.message)" -ErrorAction Continue
    exit 1
}
if ($reply.result.PSObject.Properties.Name -contains 'isError' -and $reply.result.isError) {
    Write-Error "tool $Tool returned isError" -ErrorAction Continue
    exit 1
}
exit 0
