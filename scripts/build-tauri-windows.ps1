<#
.SYNOPSIS
Release-build Gen-Audio for Windows. This is the only supported build path.

.DESCRIPTION
-Mode Full (default): NSIS and MSI installers that contain gen-audio.exe (tray
app) and the gen-audio-mcp sidecar.
-Mode NoBundle: `tauri build --no-bundle`. Same release exe with the embedded
frontendDist, no installers. Faster, for local checks.

Both modes go through the tauri CLI (pinned), so the release exe always has
the custom-protocol feature and loads the embedded UI, never devUrl.
Every native step is exit-code checked. The script prints exactly one
BUILD_OK line, and only after the outputs are verified to be from this run.

Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-tauri-windows.ps1 -Mode Full
pwsh -NoProfile -File scripts/build-tauri-windows.ps1 -Mode NoBundle
#>
[CmdletBinding()]
param(
    [ValidateSet('Full', 'NoBundle')]
    [string]$Mode = 'Full'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

$TauriCli = '@tauri-apps/cli@2.12.1'
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root 'apps\desktop'
$srcTauri = Join-Path $desktop 'src-tauri'
$releaseDir = Join-Path $root 'target\release'
# One second of slack for file systems with coarse timestamps.
$runStart = (Get-Date).AddSeconds(-1)
Set-Location $root

function Assert-Fresh {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$What)
    if (-not (Test-Path -LiteralPath $Path)) { throw "$What is missing: $Path" }
    $item = Get-Item -LiteralPath $Path
    if ($item.LastWriteTime -lt $runStart) {
        throw "$What is stale: $Path was written $($item.LastWriteTime.ToString('s')), before this run started $($runStart.ToString('s'))"
    }
    return $item
}

function Get-FreeLoopbackPort {
    $listener = New-Object System.Net.Sockets.TcpListener ([System.Net.IPAddress]::Loopback), 0
    $listener.Start()
    try { return $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}

Write-Step "build-tauri-windows.ps1 -Mode $Mode (PowerShell $($PSVersionTable.PSVersion))"
if (-not (Test-IsWindowsHost)) { throw 'build-tauri-windows.ps1 builds Windows installers; run it on Windows.' }
Import-VsDevEnv
foreach ($tool in 'cargo', 'rustc', 'npm', 'npx', 'node') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool is required on PATH" }
}

# Preflight: every file tauri.conf.json points at must exist before cargo
# runs. A missing icon used to surface as a tauri-build panic in build.rs.
$conf = Get-Content -LiteralPath (Join-Path $srcTauri 'tauri.conf.json') -Raw | ConvertFrom-Json
foreach ($icon in @($conf.bundle.icon)) {
    if (-not (Test-Path -LiteralPath (Join-Path $srcTauri $icon))) { throw "tauri.conf.json bundle.icon $icon is missing under $srcTauri" }
}
$hooks = $conf.bundle.windows.nsis.installerHooks
if ($hooks -and -not (Test-Path -LiteralPath (Join-Path $srcTauri $hooks))) { throw "NSIS installerHooks $hooks is missing" }
if ($conf.build.beforeBuildCommand -ne 'npm run build') {
    throw "tauri.conf.json build.beforeBuildCommand must be 'npm run build' (it runs in apps/desktop); found '$($conf.build.beforeBuildCommand)'"
}

$triple = ((& rustc -vV) | Select-String -Pattern '^host: ').ToString().Split(':', 2)[1].Trim()
if ($LASTEXITCODE -ne 0 -or -not $triple) { throw 'could not read the rustc host triple' }
Write-Step "host triple $triple"

Write-Step 'build gen-audio-mcp sidecar'
Invoke-Checked cargo build -p gen-audio-mcp --release
$sidecar = Assert-Fresh -Path (Join-Path $releaseDir 'gen-audio-mcp.exe') -What 'gen-audio-mcp.exe'
Write-Step "sidecar $($sidecar.FullName) $($sidecar.Length) bytes"

# Handshake on a free loopback port. 8765 may belong to an installed app.
$port = Get-FreeLoopbackPort
$addr = "127.0.0.1:$port"
Write-Step "mcp initialize handshake on $addr"
$proc = Start-Process -FilePath $sidecar.FullName -ArgumentList @('--http', $addr) -PassThru -WindowStyle Hidden
try {
    $healthy = $false
    foreach ($attempt in 1..40) {
        try {
            Invoke-WebRequest -Uri "http://$addr/health" -UseBasicParsing -TimeoutSec 2 | Out-Null
            $healthy = $true
            break
        } catch {
            if ($proc.HasExited) { throw "gen-audio-mcp exited with $($proc.ExitCode) before /health answered" }
            Start-Sleep -Milliseconds 250
        }
    }
    if (-not $healthy) { throw "gen-audio-mcp did not answer http://$addr/health" }
    $body = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"build-tauri-windows","version":"0.1.0"}}}'
    $response = Invoke-WebRequest -Uri "http://$addr/mcp" -Method POST -ContentType 'application/json' -Body $body -UseBasicParsing
    $init = $response.Content | ConvertFrom-Json
    if ($init.result.protocolVersion -ne '2025-03-26') { throw "initialize negotiated '$($init.result.protocolVersion)', expected 2025-03-26" }
    if ($init.result.serverInfo.name -ne 'gen-audio') { throw "initialize serverInfo.name was '$($init.result.serverInfo.name)', expected gen-audio" }
    Write-Step 'mcp handshake ok (2025-03-26, gen-audio)'
} finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}

# Stage the sidecar where tauri externalBin expects it. Never copy a file
# onto itself (the old script copied target\release\gen-audio-mcp.exe onto
# the same path and failed).
$binDir = Join-Path $srcTauri 'binaries'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$staged = Join-Path $binDir "gen-audio-mcp-$triple.exe"
if ([System.IO.Path]::GetFullPath($staged) -eq [System.IO.Path]::GetFullPath($sidecar.FullName)) { throw "refusing to copy $staged onto itself" }
Copy-Item -LiteralPath $sidecar.FullName -Destination $staged -Force
Write-Step "staged sidecar $staged"

Write-Step 'frontend dependencies (npm ci)'
Push-Location $desktop
try {
    Invoke-Checked npm ci
} finally {
    Pop-Location
}

# A release build never carries the dev fixture cards.
if (Test-Path Env:VITE_GEN_AUDIO_FIXTURES) {
    Write-Step 'clearing VITE_GEN_AUDIO_FIXTURES for the release build'
    Remove-Item Env:VITE_GEN_AUDIO_FIXTURES
}

# tauri build runs beforeBuildCommand (npm run build) in apps/desktop, so
# dist is rebuilt before cargo compiles the WebView.
$tauriArgs = @('--yes', $TauriCli, 'build')
if ($Mode -eq 'Full') { $tauriArgs += @('--bundles', 'nsis,msi') } else { $tauriArgs += '--no-bundle' }
Write-Step "npx $($tauriArgs -join ' ')"
Push-Location $srcTauri
try {
    Invoke-Checked npx @tauriArgs
} finally {
    Pop-Location
}

# The exe embeds dist, so dist must be from this run and carry the header.
$index = Assert-Fresh -Path (Join-Path $desktop 'dist\index.html') -What 'apps/desktop/dist/index.html'
$html = Get-Content -LiteralPath $index.FullName -Raw
foreach ($marker in 'brand-row', 'ga-header-toolbar') {
    if ($html -notmatch [regex]::Escape($marker)) { throw "dist/index.html has no '$marker'; the embedded UI is not the Gen-Audio header build" }
}
if ($html -match '/src/main\.ts') { throw 'dist/index.html references /src/main.ts; that is the Vite dev page, not a production build' }
Write-Step "frontend dist ok $($index.FullName)"

# No localhost: a dev build (no custom-protocol) makes tauri-build emit
# cargo:rustc-cfg=dev, and that exe loads devUrl http://localhost:1420.
$outputs = @(Get-ChildItem -Path (Join-Path $releaseDir 'build') -Filter 'output' -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.Directory.Name -like 'gen-audio-desktop-*' -and $_.LastWriteTime -ge $runStart })
foreach ($output in $outputs) {
    if (Select-String -LiteralPath $output.FullName -Pattern 'cargo:rustc-cfg=dev' -SimpleMatch -Quiet) {
        throw "dev build detected ($($output.FullName) has cargo:rustc-cfg=dev); this exe would load http://localhost:1420"
    }
}
Write-Step "no dev cfg in gen-audio-desktop build-script output ($($outputs.Count) checked)"

# Fingerprint gate: one desktop lib fingerprint in target\release. More than
# one means the desktop crate was built under a second configuration
# (bare cargo, another CLI, other features) and the cache is thrashing.
$fingerprints = @(Get-ChildItem -Path (Join-Path $releaseDir '.fingerprint') -Directory -Filter 'gen-audio-desktop-*' -ErrorAction SilentlyContinue |
        Where-Object { @(Get-ChildItem -LiteralPath $_.FullName -Filter 'lib-gen_audio_desktop_lib*' -ErrorAction SilentlyContinue).Count -gt 0 })
Write-Step "gen-audio-desktop lib fingerprints: $($fingerprints.Count) ($(($fingerprints | ForEach-Object { $_.Name }) -join ', '))"
if ($fingerprints.Count -ne 1) {
    throw "expected exactly one gen-audio-desktop lib fingerprint under target\release\.fingerprint, found $($fingerprints.Count)"
}

$exe = Assert-Fresh -Path (Join-Path $releaseDir 'gen-audio.exe') -What 'target\release\gen-audio.exe'

if ($Mode -eq 'Full') {
    $bundleDir = Join-Path $releaseDir 'bundle'
    $nsis = @(Get-ChildItem -Path (Join-Path $bundleDir 'nsis') -Filter '*-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending)
    $msi = @(Get-ChildItem -Path (Join-Path $bundleDir 'msi') -Filter '*.msi' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending)
    if ($nsis.Count -lt 1) { throw "no NSIS *-setup.exe under $bundleDir\nsis" }
    if ($msi.Count -lt 1) { throw "no MSI under $bundleDir\msi" }
    foreach ($installer in @($nsis[0], $msi[0])) {
        $fresh = Assert-Fresh -Path $installer.FullName -What 'installer'
        $hash = (Get-FileHash -LiteralPath $fresh.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Output "INSTALLER path=$($fresh.FullName) size=$($fresh.Length) sha256=$hash mtime=$($fresh.LastWriteTime.ToString('yyyy-MM-ddTHH:mm:sszzz'))"
    }
}

Write-Output "BUILD_OK mode=$Mode exe=$($exe.FullName) size=$($exe.Length) mtime=$($exe.LastWriteTime.ToString('yyyy-MM-ddTHH:mm:sszzz'))"
