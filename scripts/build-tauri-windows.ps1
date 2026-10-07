# Release-build Gen-Audio for Windows.
# Produces NSIS and MSI installers that contain gen-audio.exe, the tray app,
# and the gen-audio-mcp sidecar. The installed app starts that sidecar and
# stays in the notification area. Closing the window hides it. Quit is on the tray.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$File,
        [Parameter(ValueFromRemainingArguments = $true)][string[]]$Args
    )
    & $File @Args
    if ($LASTEXITCODE -ne 0) {
        throw "$File $($Args -join ' ') failed with exit $LASTEXITCODE"
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw "cargo is required" }
if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) { throw "rustc is required" }
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) { throw "npm is required" }
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw "node is required" }

$triple = (& rustc -vV | Select-String -Pattern '^host: ').ToString().Split(':', 2)[1].Trim()
if (-not $triple) { throw "could not read rustc host triple" }
Write-Host "host triple $triple"

Write-Host "build gen-audio-mcp sidecar"
Invoke-Checked cargo build -p gen-audio-mcp --release
$sidecar = Join-Path $root "target\release\gen-audio-mcp.exe"
if (-not (Test-Path $sidecar)) { throw "missing $sidecar" }

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

$binDir = Join-Path $root "apps\desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$staged = Join-Path $binDir "gen-audio-mcp-$triple.exe"
Copy-Item -Force $sidecar $staged
Write-Host "staged sidecar $staged"

Write-Host "frontend"
Push-Location (Join-Path $root "apps\desktop")
try {
    if (Test-Path "package-lock.json") {
        Invoke-Checked npm ci
    } else {
        Invoke-Checked npm install
    }
    Invoke-Checked npm run build
} finally {
    Pop-Location
}

$index = Join-Path $root "apps\desktop\dist\index.html"
if (-not (Test-Path $index)) {
    throw "apps/desktop/dist/index.html is missing. Refusing to compile the desktop WebView (that exe would load http://localhost:1420)."
}
Write-Host "frontend dist ok $index"

Write-Host "package NSIS and MSI (tauri build embeds frontendDist; no bare desktop cargo build)"
Push-Location (Join-Path $root "apps\desktop\src-tauri")
try {
    Invoke-Checked npx --yes "@tauri-apps/cli" build --bundles nsis,msi
} finally {
    Pop-Location
}

$bundleRoots = @(
    (Join-Path $root "target\release\bundle"),
    (Join-Path $root "apps\desktop\src-tauri\target\release\bundle")
)
$nsis = @()
$msi = @()
foreach ($dir in $bundleRoots) {
    if (Test-Path $dir) {
        $nsis += @(Get-ChildItem -Path $dir -Recurse -Filter "*setup.exe" -ErrorAction SilentlyContinue)
        $msi += @(Get-ChildItem -Path $dir -Recurse -Filter "*.msi" -ErrorAction SilentlyContinue)
    }
}
if ($nsis.Count -lt 1) { throw "NSIS setup.exe was not produced under target\release\bundle" }
if ($msi.Count -lt 1) { throw "MSI was not produced under target\release\bundle" }

Write-Host "NSIS $($nsis[0].FullName)"
Write-Host "MSI $($msi[0].FullName)"
Write-Host "Ship the MSI or NSIS installer. Do not launch a gen-audio-desktop.exe from a bare cargo build; that binary loads http://localhost:1420."
Write-Host "build-tauri-windows.ps1 finished"
