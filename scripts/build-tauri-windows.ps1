# Release-build Gen-Audio for Windows: native app, gen-audio-mcp sidecar,
# NSIS and MSI packages, then an MCP initialize handshake.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is required"
}

Write-Host "build gen-audio-mcp sidecar"
cargo build -p gen-audio-mcp --release
$sidecar = Join-Path $root "target\release\gen-audio-mcp.exe"
if (-not (Test-Path $sidecar)) {
    throw "missing $sidecar"
}

Write-Host "mcp initialize handshake"
$addr = "127.0.0.1:8765"
$proc = Start-Process -FilePath $sidecar -ArgumentList @("--http", $addr) -PassThru -WindowStyle Hidden
try {
    $ok = $false
    foreach ($i in 1..20) {
        try {
            Invoke-WebRequest -Uri "http://$addr/health" -UseBasicParsing -TimeoutSec 2 | Out-Null
            $ok = $true
            break
        } catch {
            Start-Sleep -Milliseconds 250
        }
    }
    if (-not $ok) { throw "gen-audio-mcp did not answer /health" }
    $body = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"build","version":"0.1.0"}}}'
    $response = Invoke-WebRequest -Uri "http://$addr/mcp" -Method POST -ContentType "application/json" -Body $body -UseBasicParsing
    if ($response.Content -notmatch '"protocolVersion":"2025-03-26"') {
        throw "initialize did not negotiate 2025-03-26"
    }
    if ($response.Content -notmatch '"name":"gen-audio"') {
        throw "initialize serverInfo.name was not gen-audio"
    }
    Write-Host "mcp handshake ok"
} finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}

Write-Host "desktop libraries (tray + embedded mcp)"
cargo build -p gen-audio-desktop --release

if (Get-Command npm -ErrorAction SilentlyContinue) {
    Push-Location (Join-Path $root "apps\desktop")
    npm ci
    npm run build
    Pop-Location
}

Write-Host "package NSIS and MSI"
Push-Location (Join-Path $root "apps\desktop\src-tauri")
npx --yes @tauri-apps/cli build --bundles nsis,msi
Pop-Location

Copy-Item -Force $sidecar (Join-Path $root "target\release\gen-audio-mcp.exe")
Write-Host "build-tauri-windows.ps1 finished"
