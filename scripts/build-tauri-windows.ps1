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
Every native step is exit-code checked. Before any build step, every library
WAV named in schemas/asset-object/media.lock.json must be staged with the
locked sha256 and byte count (they are gitignored, and the release embeds
them). The script prints exactly one BUILD_OK line, and only after the
outputs are verified by content (sha256, never mtime). Every run, failed ones included,
saves its full transcript to target/logs/build-<sha>-<timestamp>.log; the
first output line (BUILD_LOG) and BUILD_OK (log=) name it.

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
Set-Location $root

function Get-FreeLoopbackPort {
    $listener = New-Object System.Net.Sockets.TcpListener ([System.Net.IPAddress]::Loopback), 0
    $listener.Start()
    try { return $listener.LocalEndpoint.Port } finally { $listener.Stop() }
function Invoke-Checked {
    # Simple function on purpose. A param() block with [Parameter()] is an
    # advanced function, so PowerShell binds a single-dash token itself:
    # -p is -PipelineVariable on Windows PowerShell 5.1 (cargo never sees the
    # package) and is ambiguous with -ProgressAction on PowerShell 7.4+
    # (the call throws before cargo starts). $args forwards every token.
    if ($args.Count -lt 1) { throw "Invoke-Checked requires a command" }
    $file = [string]$args[0]
    $commandArgs = [string[]]@($args | Select-Object -Skip 1)
    & $file @commandArgs
    if ($LASTEXITCODE -ne 0) {
        throw "$file $($commandArgs -join ' ') failed with exit $LASTEXITCODE"
    }
}

# The whole run, native tool output included (Invoke-Checked and
# Invoke-CargoBinBuild send it through the host), is saved to
# target/logs/build-<HEAD sha>-<timestamp>.log for the stamp trail. The
# transcript stops in finally, so a failed build leaves its log too.
$buildLog = Get-BuildLogPath -Root $root
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $buildLog) | Out-Null
Start-Transcript -LiteralPath $buildLog -Force | Out-Null
Write-Output "BUILD_LOG $buildLog"
try {
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

    # The release embeds apps/desktop/public/library through frontendDist, and
    # *.wav is gitignored: a fresh worktree has none. SMAX printed BUILD_OK with
    # zero WAVs and the installed app failed with 'NotSupportedError: no
    # supported sources'. Every WAV in media.lock.json must be staged with the
    # locked bytes, or the build stops here (no skip).
    $wavProblems = @(Get-LibraryWavProblem -Root $root)
    if ($wavProblems.Count -gt 0) {
        $wavProblems | ForEach-Object { Write-Output $_ }
        throw "$($wavProblems.Count) library WAV(s) missing or not the media.lock.json bytes; stage them in apps\desktop\public\library before a release build"
    }
    Write-Step 'library WAVs match media.lock.json (sha256, bytes)'

    $triple = ((& rustc -vV) | Select-String -Pattern '^host: ').ToString().Split(':', 2)[1].Trim()
    if ($LASTEXITCODE -ne 0 -or -not $triple) { throw 'could not read the rustc host triple' }
    Write-Step "host triple $triple"

    # Sidecar freshness is cargo's fingerprint verdict plus a content hash, never
    # the file mtime: a no-op rebuild does not rewrite gen-audio-mcp.exe, so the
    # old mtime check called an up-to-date binary stale (SMAX had to run a scoped
    # clean of the two crates to get past it).
    Write-Step 'build gen-audio-mcp sidecar'
    $artifact = Invoke-CargoBinBuild -Package gen-audio-mcp -Bin gen-audio-mcp -ExtraArgs @('--release')
    $sidecar = Get-Item -LiteralPath $artifact.Executable
    $sidecarHash = Get-Sha256 -LiteralPath $sidecar.FullName
    Write-Step "sidecar $($sidecar.FullName) $($sidecar.Length) bytes sha256 $sidecarHash; cargo: $(if ($artifact.Fresh) { 'up to date (fingerprint fresh)' } else { 'rebuilt' })"

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
    $stagedHash = Get-Sha256 -LiteralPath $staged
    if ($stagedHash -ne $sidecarHash) { throw "staged sidecar $staged has sha256 $stagedHash, but cargo's binary has $sidecarHash" }
    Write-Step "staged sidecar $staged (sha256 $stagedHash, same bytes as cargo's binary)"

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

    # Outputs are judged by content, never mtime: a no-op rebuild leaves an
    # up-to-date gen-audio.exe with its old mtime, which the old mtime check
    # called stale. Fingerprint them before the build, compare after.
    # Installers are this build's version (tauri.conf.json), not the newest file.
    $installerGlob = @{ nsis = "*_$($conf.version)_*-setup.exe"; msi = "*_$($conf.version)_*.msi" }
    $indexPath = Join-Path $desktop 'dist\index.html'
    $exePath = Join-Path $releaseDir 'gen-audio.exe'
    $before = @{ index = Get-OutputFingerprint -LiteralPath $indexPath; exe = Get-OutputFingerprint -LiteralPath $exePath }
    foreach ($kind in 'nsis', 'msi') {
        foreach ($old in @(Get-ChildItem -Path (Join-Path $releaseDir "bundle\$kind") -Filter $installerGlob[$kind] -ErrorAction SilentlyContinue)) {
            $before[$old.FullName] = Get-OutputFingerprint -LiteralPath $old.FullName
        }
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

    # The exe embeds dist, so dist must hold this tree's public files byte for
    # byte, and an index.html with the header whose script and style assets exist.
    $index = Assert-BuildOutput -LiteralPath $indexPath -What 'apps/desktop/dist/index.html' -Before $before.index
    $drift = @(Get-DistCopyDrift -Source (Join-Path $desktop 'public') -Dist (Join-Path $desktop 'dist'))
    if ($drift.Count -gt 0) {
        $drift | ForEach-Object { Write-Output $_ }
        throw "dist does not hold this tree's apps/desktop/public ($($drift.Count) file(s)); the embedded UI would be stale"
    }
    $html = Get-Content -LiteralPath $index.Path -Raw
    foreach ($ref in @([regex]::Matches($html, '(?:src|href)="/(assets/[^"]+)"') | ForEach-Object { $_.Groups[1].Value })) {
        if (-not (Test-Path -LiteralPath (Join-Path (Join-Path $desktop 'dist') $ref) -PathType Leaf)) { throw "dist/index.html references /$ref, which is not in dist" }
    }
    foreach ($marker in 'brand-row', 'ga-header-toolbar') {
        if ($html -notmatch [regex]::Escape($marker)) { throw "dist/index.html has no '$marker'; the embedded UI is not the Gen-Audio header build" }
    }
    if ($html -match '/src/main\.ts') { throw 'dist/index.html references /src/main.ts; that is the Vite dev page, not a production build' }
    # Release ships the Library deck only: no dev fixture assets, no example deck chunk.
    $distAssets = Get-Content -Raw -LiteralPath (Join-Path $desktop 'dist\library\assets.json')
    foreach ($claim in 'fixture_tone', 'reference_only') {
        if ($distAssets -match [regex]::Escape("`"$claim`"")) { throw "dist/library/assets.json carries a dev fixture ($claim); release must not ship spec-fixture, cube-fixture or bench-ref" }
    }
    $exampleChunk = @(Get-ChildItem -LiteralPath (Join-Path $desktop 'dist\assets') -Filter 'viewport.example-*.js' -ErrorAction SilentlyContinue)
    if ($exampleChunk.Count -gt 0) { throw "dist has $($exampleChunk[0].Name); VITE_GEN_AUDIO_FIXTURES leaked into a release build" }
    Write-Step "frontend dist ok $($index.Path) sha256 $($index.Sha256) ($($index.State); public files match; no dev fixtures)"

    # No localhost: a dev build (no custom-protocol) makes tauri-build emit
    # cargo:rustc-cfg=dev, and that exe loads devUrl http://localhost:1420.
    # Every gen-audio-desktop build-script output is checked, whatever its
    # mtime (a no-op rebuild does not rewrite it; the fingerprint gate below
    # allows only one desktop configuration).
    $outputs = @(Get-ChildItem -Path (Join-Path $releaseDir 'build') -Filter 'output' -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -like 'gen-audio-desktop-*' })
    foreach ($output in $outputs) {
        if (Select-String -LiteralPath $output.FullName -Pattern 'cargo:rustc-cfg=dev' -SimpleMatch -Quiet) {
            throw "dev build detected ($($output.FullName) has cargo:rustc-cfg=dev); this exe would load http://localhost:1420"
        }
    }
    Write-Step "no dev cfg in gen-audio-desktop build-script output ($($outputs.Count) checked)"
Write-Host "package NSIS and MSI (tauri build embeds frontendDist; no bare desktop cargo build)"
Push-Location (Join-Path $root "apps\desktop\src-tauri")
try {
    # Same Tauri version as the tauri crate in Cargo.lock. An unpinned npx
    # package resolves latest on every build.
    Invoke-Checked npx --yes "@tauri-apps/cli@2.12.1" build --bundles "nsis,msi"
} finally {
    Pop-Location
}

    # Fingerprint gate: one desktop lib fingerprint in target\release. More than
    # one means the desktop crate was built under a second configuration
    # (bare cargo, another CLI, other features) and the cache is thrashing.
    $fingerprints = @(Get-ChildItem -Path (Join-Path $releaseDir '.fingerprint') -Directory -Filter 'gen-audio-desktop-*' -ErrorAction SilentlyContinue |
            Where-Object { @(Get-ChildItem -LiteralPath $_.FullName -Filter 'lib-gen_audio_desktop_lib*' -ErrorAction SilentlyContinue).Count -gt 0 })
    Write-Step "gen-audio-desktop lib fingerprints: $($fingerprints.Count) ($(($fingerprints | ForEach-Object { $_.Name }) -join ', '))"
    if ($fingerprints.Count -ne 1) {
        throw "expected exactly one gen-audio-desktop lib fingerprint under target\release\.fingerprint, found $($fingerprints.Count)"
    }
}
if ($nsis.Count -lt 1) { throw "NSIS setup.exe was not produced under target\release\bundle" }
if ($msi.Count -lt 1) { throw "MSI was not produced under target\release\bundle" }
$nsis = @($nsis | Sort-Object -Property LastWriteTime -Descending)
$msi = @($msi | Sort-Object -Property LastWriteTime -Descending)

    $exe = Assert-BuildOutput -LiteralPath $exePath -What 'target\release\gen-audio.exe' -Before $before.exe
    Write-Step "exe $($exe.Path) sha256 $($exe.Sha256) ($($exe.State))"

    if ($Mode -eq 'Full') {
        $bundleDir = Join-Path $releaseDir 'bundle'
        foreach ($kind in 'nsis', 'msi') {
            $found = @(Get-ChildItem -Path (Join-Path $bundleDir $kind) -Filter $installerGlob[$kind] -ErrorAction SilentlyContinue)
            if ($found.Count -ne 1) { throw "expected one $kind installer $($installerGlob[$kind]) (version $($conf.version)) under $bundleDir\$kind, found $($found.Count): $(($found | ForEach-Object { $_.Name }) -join ', ')" }
            $installer = $found[0].FullName
            $built = Assert-BuildOutput -LiteralPath $installer -What "$kind installer" -Before $before[$installer]
            Write-Output "INSTALLER path=$($built.Path) size=$($built.Length) sha256=$($built.Sha256) state=$($built.State)"
        }
    }

    Write-Output "BUILD_OK mode=$Mode exe=$($exe.Path) size=$($exe.Length) sha256=$($exe.Sha256) state=$($exe.State) log=$buildLog"
} catch {
    # Record the failure inside the transcript before it closes, then rethrow.
    Write-Output "BUILD_FAIL log=$buildLog $($_.Exception.Message)"
    throw
} finally {
    Stop-Transcript | Out-Null
}
