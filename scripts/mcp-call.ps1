<#
.SYNOPSIS
Call one Gen-Audio MCP tool over HTTP JSON-RPC (tools/call) and print the result.

.DESCRIPTION
Requires GEN_AUDIO_MCP_TOKEN (64 hex digits) in the process environment, or
-ConnectionFile / GEN_AUDIO_MCP_CONNECTION_FILE naming the desktop's private
transient connection export (addr, token, handshake_ok). Never put the token
in command arguments or a URL. A connection export pins discovery to its
owned address. Otherwise finds the server the same way the desktop binds it,
then checks that the
port really is Gen-Audio (initialize, serverInfo.name = gen-audio):
  1. -Port, when given
  2. GEN_AUDIO_MCP_ADDR (host:port), the desktop app's preferred address
  3. 127.0.0.1:8765, the default
  4. Windows only: loopback ports a gen-audio / gen-audio-mcp process is
     listening on (the app falls back to an ephemeral port when 8765 is taken)
Prints `MCP addr=<host:port> tool=<name>` and then the JSON-RPC result.
Exits non-zero on a JSON-RPC error or a tool result with isError true.

-OutFile <path> writes only the JSON-RPC response body to <path> (UTF-8, no
BOM, the bytes as received), also when the response is a JSON-RPC error, so
the evidence file is machine-checkable (ConvertFrom-Json, python json.load).
With -OutFile the `MCP addr=... tool=...` header and a final `exit=<N>` line go
to stderr and nothing goes to stdout; Write-Error stays on the error stream and
the exit code is the same as without it. Without -OutFile the header and the
body go to stdout as before.
Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
pwsh -NoProfile -File scripts/mcp-call.ps1 -Tool ui_navigate -ArgsJson '{"slide":"slide:library"}'

.EXAMPLE
pwsh -NoProfile -File scripts/mcp-call.ps1 -Tool viewport_get -OutFile artifacts/viewport.json
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Tool,
    [string]$ArgsJson = '{}',
    [ValidateRange(0, 65535)]
    [int]$Port = 0,
    [ValidateRange(1, 3600)]
    [int]$TimeoutSec = 15,
    # Write only the JSON-RPC response body here (UTF-8, no BOM); header and exit=<N> go to stderr.
    [string]$OutFile = '',
    [string]$ConnectionFile = $env:GEN_AUDIO_MCP_CONNECTION_FILE
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

function Invoke-JsonRpc {
    param([Parameter(Mandatory = $true)][string]$Address, [Parameter(Mandatory = $true)][string]$Body, [int]$Timeout = 5)
    try {
        $response = Invoke-WebRequest -Uri "http://$Address/mcp" -Method POST -ContentType 'application/json' -Body ([System.Text.Encoding]::UTF8.GetBytes($Body)) -Headers $script:authHeaders -MaximumRedirection 0 -UseBasicParsing -TimeoutSec $Timeout -Verbose:$false
    } catch {
        # Do not echo an HTTP error body or headers that could contain auth.
        throw "MCP HTTP request failed at $Address (check the owned connection and its authentication)"
    }
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
        return ($reply.result.serverInfo.name -eq 'gen-audio' -and $reply.result.protocolVersion -eq '2025-03-26')
    } catch {
        Write-Verbose "no Gen-Audio MCP at ${Address}: $($_.Exception.Message)"
        return $false
    }
}

function Test-LoopbackAddress {
    [OutputType([bool])]
    param([string]$Address)
    return ($Address -match '\A(?:127\.0\.0\.1|localhost|\[::1\]):([1-9][0-9]{0,4})\z' -and [int]$Matches[1] -le 65535)
}

$token = $env:GEN_AUDIO_MCP_TOKEN
$ownedAddress = $null
if ($ConnectionFile) {
    try {
        $file = Get-Item -LiteralPath $ConnectionFile
        if ($file.Length -le 0 -or $file.Length -gt 65536) { throw 'invalid size' }
        $connection = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($connection.handshake_ok -isnot [bool] -or -not $connection.handshake_ok -or -not (Test-LoopbackAddress -Address $connection.addr)) { throw 'invalid connection' }
        $token = [string]$connection.token
        $ownedAddress = [string]$connection.addr
    } catch {
        throw 'the connection file must be a usable private desktop connection export'
    }
    if ($Port -gt 0 -and $ownedAddress -ne "127.0.0.1:$Port") { throw '-Port conflicts with the owned connection export; omit -Port' }
}
if ($token -notmatch '\A[0-9a-fA-F]{64}\z') { throw 'set GEN_AUDIO_MCP_TOKEN to the owned server bearer (64 hex digits), or supply -ConnectionFile; never pass credentials in a URL' }
$script:authHeaders = @{ Authorization = "Bearer $token" }
$token = $null

try {
    $arguments = $ArgsJson | ConvertFrom-Json
} catch {
    throw "-ArgsJson is not valid JSON: $ArgsJson"
}
if ($null -eq $arguments -or $arguments -is [array] -or $arguments -is [string] -or $arguments -is [ValueType]) {
    throw "-ArgsJson must be a JSON object, got: $ArgsJson"
}

$candidates = New-Object System.Collections.Generic.List[string]
if ($ownedAddress) {
    $candidates.Add($ownedAddress)
} elseif ($Port -gt 0) {
    $candidates.Add("127.0.0.1:$Port")
} else {
    if ($env:GEN_AUDIO_MCP_ADDR) {
        if (-not (Test-LoopbackAddress -Address $env:GEN_AUDIO_MCP_ADDR)) { throw 'GEN_AUDIO_MCP_ADDR must be a loopback host:port without credentials, path or query' }
        $candidates.Add($env:GEN_AUDIO_MCP_ADDR)
    }
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
if ($OutFile) {
    # The file holds the body and nothing else; the header goes to stderr.
    $outPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutFile)
    [System.IO.File]::WriteAllText($outPath, $text, (New-Object System.Text.UTF8Encoding($false)))
    [Console]::Error.WriteLine("MCP addr=$addr tool=$Tool")
} else {
    Write-Output "MCP addr=$addr tool=$Tool"
    Write-Output $text
}
$code = 0
$reply = $text | ConvertFrom-Json
if ($reply.PSObject.Properties.Name -contains 'error') {
    Write-Error "JSON-RPC error $($reply.error.code): $($reply.error.message)" -ErrorAction Continue
    $code = 1
} elseif ($reply.result.PSObject.Properties.Name -contains 'isError' -and $reply.result.isError) {
    Write-Error "tool $Tool returned isError" -ErrorAction Continue
    $code = 1
}
if ($OutFile) { [Console]::Error.WriteLine("exit=$code") }
exit $code
