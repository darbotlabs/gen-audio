<#
.SYNOPSIS
Release-build Gen-Audio for Windows. This is the only supported build path.

.DESCRIPTION
-Mode Full (default): NSIS and MSI installers containing gen-audio.exe and
the gen-audio-mcp sidecar. -Mode NoBundle (or -NoBundle): the same release
exe with embedded frontendDist, without installers.
-Mode Msix builds the release exe and a signed MSIX containing the desktop,
embedded frontend and MCP sidecar. -Install installs and verifies that MSIX.
-Mode Msi produces only the production Windows MSI installer.

Both modes use the package-lock pinned local Tauri CLI and locked Cargo
dependencies, with two build jobs. -Clean archives existing deliverables,
cleans repository Cargo outputs, disables compiler wrappers/incremental builds,
and installs frontend dependencies using an empty temporary npm cache.
Before building, every library WAV must match media.lock.json (sha256 and
bytes). The built sidecar runs its own --smoke without binding an HTTP port.
Outputs are checked by content, never mtime; up-to-date outputs are valid.
Every run saves a transcript to target/logs/build-<sha>-<timestamp>.log.
BUILD_LOG names it first; BUILD_OK is emitted only after all checks pass.

Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-tauri-windows.ps1 -Mode Full
pwsh -NoProfile -File scripts\build-tauri-windows.ps1 -NoBundle
powershell -NoProfile -File scripts\build-tauri-windows.ps1 -Mode Msix -Clean -Install
powershell -NoProfile -File scripts\build-tauri-windows.ps1 -Mode Msi -Clean
#>
[CmdletBinding()]
param(
    [ValidateSet('Full', 'Msi', 'NoBundle', 'Msix')]
    [string]$Mode = 'Full',
    [switch]$NoBundle,
    [switch]$Clean,
    [switch]$Install,
    [string]$SigningCertificateThumbprint
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')
. (Join-Path $PSScriptRoot 'lib\msix.ps1')

if ($NoBundle) { $Mode = 'NoBundle' }
if ($Install -and $Mode -ne 'Msix') { throw '-Install requires -Mode Msix' }
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root 'apps\desktop'
$srcTauri = Join-Path $desktop 'src-tauri'
$savedTargetDir = $env:CARGO_TARGET_DIR
$savedBuildJobs = $env:CARGO_BUILD_JOBS
$savedFixtures = $env:VITE_GEN_AUDIO_FIXTURES
$savedIncremental = $env:CARGO_INCREMENTAL
$savedRustWrapper = $env:RUSTC_WRAPPER
$savedWorkspaceWrapper = $env:RUSTC_WORKSPACE_WRAPPER
$savedSignToolPath = $env:TAURI_WINDOWS_SIGNTOOL_PATH
$freshNpmCache = $null
$cleanArchive = $null

$buildLog = Get-BuildLogPath -Root $root
if ($Clean) {
    $cleanArchive = Join-Path (Join-Path $root 'artifacts\builds') ([IO.Path]::GetFileNameWithoutExtension($buildLog))
    $buildLog = Join-Path $cleanArchive 'build.log'
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $buildLog) | Out-Null
Start-Transcript -LiteralPath $buildLog -Force | Out-Null
Write-Output "BUILD_LOG $buildLog"
Push-Location $root
try {
    Write-Step "build-tauri-windows.ps1 -Mode $Mode (PowerShell $($PSVersionTable.PSVersion))"
    if (-not (Test-IsWindowsHost)) { throw 'build-tauri-windows.ps1 builds Windows installers; run it on Windows.' }
    $env:CARGO_BUILD_JOBS = '2'
    if ($env:CARGO_TARGET_DIR -and -not [IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        $env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath((Join-Path $root $env:CARGO_TARGET_DIR))
    }
    foreach ($tool in 'cargo', 'rustc', 'npm.cmd', 'node') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool is required on PATH" }
    }
    $signingCertificate = $null
    if ($Mode -eq 'Msix') {
        Get-WindowsSdkTool -Name makeappx.exe | Out-Null
        Get-WindowsSdkTool -Name signtool.exe | Out-Null
        $signingCertificate = Get-MsixSigningCertificate -Thumbprint $SigningCertificateThumbprint
        Write-Step "MSIX publisher $($signingCertificate.Subject); signing certificate $($signingCertificate.Thumbprint)"
    } elseif ($SigningCertificateThumbprint) {
        $signingCertificate = Get-MsixSigningCertificate -Thumbprint $SigningCertificateThumbprint
        Get-WindowsSdkTool -Name signtool.exe | Out-Null
    }
    if ($signingCertificate) {
        # Tauri also uses this tool for pre-signature verification of DLLs and
        # sidecars, even when signCommand is configured explicitly.
        $env:TAURI_WINDOWS_SIGNTOOL_PATH = Get-WindowsSdkTool -Name signtool.exe
    }

    $conf = Get-Content -LiteralPath (Join-Path $srcTauri 'tauri.conf.json') -Raw | ConvertFrom-Json
    $runtimePayload = $null
    if ($conf.bundle.PSObject.Properties['resources']) {
        if ($conf.bundle.resources.'gen/runtime/' -ne 'runtime/') { throw 'Installed runtime resources must map gen/runtime/ to runtime/' }
        $runtimePayload = New-RuntimePayload -Root $root
        Write-Step 'installed runtime source and curated assets staged and hash-verified'
    }
    foreach ($icon in @($conf.bundle.icon)) {
        if (-not (Test-Path -LiteralPath (Join-Path $srcTauri $icon) -PathType Leaf)) { throw "tauri.conf.json bundle.icon $icon is missing under $srcTauri" }
    }
    $hooks = $conf.bundle.windows.nsis.installerHooks
    if ($hooks -and -not (Test-Path -LiteralPath (Join-Path $srcTauri $hooks) -PathType Leaf)) { throw "NSIS installerHooks $hooks is missing" }
    if ($conf.build.beforeBuildCommand -ne 'npm run build') {
        throw "tauri.conf.json build.beforeBuildCommand must be 'npm run build' (it runs in apps/desktop); found '$($conf.build.beforeBuildCommand)'"
    }

    # The release embeds these bytes; missing or edited media must fail before
    # either Cargo or the frontend can produce a seemingly successful build.
    $wavProblems = @(Get-LibraryWavProblem -Root $root)
    if ($wavProblems.Count -gt 0) {
        $wavProblems | ForEach-Object { Write-Output $_ }
        throw "$($wavProblems.Count) library WAV(s) missing or not the media.lock.json bytes; stage them in apps\desktop\public\library before a release build"
    }
    Write-Step 'library WAVs match media.lock.json (sha256, bytes)'

    if ($Clean) {
        $cleanTarget = [IO.Path]::GetFullPath((Join-Path $root 'target'))
        if ($env:CARGO_TARGET_DIR -and [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) -ne $cleanTarget) { throw '-Clean requires the repository target directory; unset CARGO_TARGET_DIR first' }
        if (Test-Path -LiteralPath $cleanTarget) {
            if ((Resolve-Path -LiteralPath $cleanTarget).Path -ne $cleanTarget -or (Get-Item -LiteralPath $cleanTarget).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing to clean a redirected Cargo target directory' }
            foreach ($relative in @('release\bundle', 'logs')) {
                $existing = Join-Path $cleanTarget $relative
                if (-not (Test-Path -LiteralPath $existing)) { continue }
                $backup = Join-Path $cleanArchive ('preserved-artifacts\' + $relative)
                New-Item -ItemType Directory -Force -Path $backup | Out-Null
                foreach ($file in @(Get-ChildItem -LiteralPath $existing -Recurse -File)) {
                    $saved = Join-Path $backup $file.FullName.Substring($existing.Length + 1)
                    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $saved) | Out-Null
                    Copy-Item -LiteralPath $file.FullName -Destination $saved
                    if ((Get-Sha256 -LiteralPath $saved) -ne (Get-Sha256 -LiteralPath $file.FullName)) { throw "Archive verification failed: $($file.Name)" }
                }
            }
        }
        $env:CARGO_TARGET_DIR = $cleanTarget
        $env:CARGO_INCREMENTAL = '0'
        $env:RUSTC_WRAPPER = ''
        $env:RUSTC_WORKSPACE_WRAPPER = ''
        Write-Step "clean Cargo output $cleanTarget; existing installers/logs preserved under $cleanArchive"
        Invoke-Checked cargo clean --target-dir $cleanTarget
        $cleanDist = [IO.Path]::GetFullPath((Join-Path $desktop 'dist'))
        if (Test-Path -LiteralPath $cleanDist) {
            if ((Resolve-Path -LiteralPath $cleanDist).Path -ne $cleanDist -or (Get-Item -LiteralPath $cleanDist).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing to clean a redirected frontend dist directory' }
            Remove-Item -LiteralPath $cleanDist -Recurse -Force
        }
        $freshNpmCache = Join-Path ([IO.Path]::GetFullPath([IO.Path]::GetTempPath())) ('gen-audio-npm-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $freshNpmCache | Out-Null
    }

    $hostLines = @((& rustc -vV) | Select-String -Pattern '^host: ')
    if ($LASTEXITCODE -ne 0 -or $hostLines.Count -ne 1) { throw 'could not read the rustc host triple' }
    $triple = $hostLines[0].Line.Substring(6).Trim()
    if ($triple -notmatch '^(x86_64|aarch64|i686)-pc-windows-msvc$') { throw "a native Windows MSVC toolchain is required; found $triple" }
    Write-Step "host triple $triple"
    $architecture = switch ($triple.Split('-')[0]) { 'aarch64' { 'arm64' } 'i686' { 'x86' } default { 'x64' } }
    Import-VsDevEnv -Architecture $architecture

    # Cargo's fingerprint verdict and the artifact path are authoritative even
    # with CARGO_TARGET_DIR or a no-op rebuild that leaves an old file mtime.
    Write-Step 'build gen-audio-mcp sidecar'
    $artifact = Invoke-CargoBinBuild -Package gen-audio-mcp -Bin gen-audio-mcp -ExtraArgs @('--release')
    $sidecar = Get-Item -LiteralPath $artifact.Executable
    if ($runtimePayload) {
        $redist = Join-Path $srcTauri 'gen\redist'
        if ((Test-Path -LiteralPath $redist) -and (Get-Item -LiteralPath $redist).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing a redirected CRT staging directory' }
        New-Item -ItemType Directory -Force -Path $redist | Out-Null
        $crt = @(Get-ChildItem -LiteralPath (Join-Path $env:VCToolsRedistDir $architecture) -Filter 'vcruntime140.dll' -Recurse -File | Select-Object -First 1)
        if ($crt.Count -ne 1) { throw 'The app-local Visual C++ runtime is missing from the selected MSVC toolchain' }
        $crtPath = Join-Path $redist 'vcruntime140.dll'
        Copy-Item -LiteralPath $crt[0].FullName -Destination $crtPath -Force
        if ((Get-Sha256 -LiteralPath $crtPath) -ne (Get-Sha256 -LiteralPath $crt[0].FullName)) { throw 'App-local CRT copy differs from the selected toolchain' }
        Write-Step 'app-local Visual C++ runtime staged and hash-verified'
    }
    $releaseDir = $sidecar.Directory.FullName
    $sidecarHash = Get-Sha256 -LiteralPath $sidecar.FullName
    Write-Step "sidecar $($sidecar.FullName) $($sidecar.Length) bytes sha256 $sidecarHash; cargo: $(if ($artifact.Fresh) { 'up to date (fingerprint fresh)' } else { 'rebuilt' })"

    # Exercise this executable, not an installed app's listener. The app rejects
    # scratch inside its repository, so issue a private, unique temporary folder.
    $smokeBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $smokeDir = [IO.Path]::GetFullPath((Join-Path $smokeBase ("gen-audio-build-smoke-" + [guid]::NewGuid().ToString('N'))))
    if (-not $smokeDir.StartsWith($smokeBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'smoke scratch escapes the temporary directory' }
    New-Item -ItemType Directory -Path $smokeDir | Out-Null
    $savedWorkDir = $env:GEN_AUDIO_WORK_DIR
    try {
        $env:GEN_AUDIO_WORK_DIR = $smokeDir
        Invoke-Checked $sidecar.FullName --smoke
    } finally {
        $env:GEN_AUDIO_WORK_DIR = $savedWorkDir
        $resolvedSmoke = (Resolve-Path -LiteralPath $smokeDir).Path
        if ($resolvedSmoke -ne $smokeDir -or -not $resolvedSmoke.StartsWith($smokeBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'refusing cleanup of an unexpected smoke directory' }
        Remove-Item -LiteralPath $resolvedSmoke -Recurse -Force
    }

    $binDir = Join-Path $srcTauri 'binaries'
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    $staged = Join-Path $binDir "gen-audio-mcp-$triple.exe"
    if ([IO.Path]::GetFullPath($staged) -eq [IO.Path]::GetFullPath($sidecar.FullName)) { throw "refusing to copy $staged onto itself" }
    Copy-Item -LiteralPath $sidecar.FullName -Destination $staged -Force
    $stagedHash = Get-Sha256 -LiteralPath $staged
    if ($stagedHash -ne $sidecarHash) { throw "staged sidecar $staged has sha256 $stagedHash, but cargo's binary has $sidecarHash" }
    Write-Step "staged sidecar $staged (sha256 $stagedHash, same bytes as cargo's binary)"

    Write-Step 'frontend dependencies (npm ci)'
    Push-Location $desktop
    try {
        if ($Clean) { Invoke-Checked npm.cmd ci --no-audit --no-fund --cache $freshNpmCache --prefer-online }
        else { Invoke-Checked npm.cmd ci --no-audit --no-fund }
    } finally {
        Pop-Location
    }
    $package = Get-Content -LiteralPath (Join-Path $desktop 'package.json') -Raw | ConvertFrom-Json
    $cliVersion = $package.devDependencies.'@tauri-apps/cli'
    if ($cliVersion -notmatch '^\d+\.\d+\.\d+$' -or $package.scripts.tauri -ne 'tauri') { throw 'package.json must pin the local Tauri CLI and expose npm run tauri' }
    if (-not (Test-Path -LiteralPath (Join-Path $desktop 'node_modules\.bin\tauri.cmd') -PathType Leaf)) { throw 'the locked local Tauri CLI was not installed' }
    $installedCli = Get-Content -LiteralPath (Join-Path $desktop 'node_modules\@tauri-apps\cli\package.json') -Raw | ConvertFrom-Json
    if ($installedCli.version -ne $cliVersion) { throw 'the installed Tauri CLI does not match the pinned package version' }

    if (Test-Path Env:VITE_GEN_AUDIO_FIXTURES) {
        Write-Step 'clearing VITE_GEN_AUDIO_FIXTURES for the release build'
        Remove-Item Env:VITE_GEN_AUDIO_FIXTURES
    }

    $installerGlob = @{ nsis = "*_$($conf.version)_*-setup.exe"; msi = "*_$($conf.version)_*.msi" }
    $indexPath = Join-Path $desktop 'dist\index.html'
    $exePath = Join-Path $releaseDir 'gen-audio.exe'
    $before = @{ index = Get-OutputFingerprint -LiteralPath $indexPath; exe = Get-OutputFingerprint -LiteralPath $exePath }
    foreach ($kind in 'nsis', 'msi') {
        foreach ($old in @(Get-ChildItem -Path (Join-Path $releaseDir "bundle\$kind") -Filter $installerGlob[$kind] -File -ErrorAction SilentlyContinue)) {
            $before[$old.FullName] = Get-OutputFingerprint -LiteralPath $old.FullName
        }
    }

    # The one Tauri invocation runs beforeBuildCommand before compiling the
    # embedded UI. npm resolves the already installed CLI, never a download.
    $tauriArgs = @('run', 'tauri', '--', 'build')
    if ($Mode -eq 'Full') { $tauriArgs += @('--bundles', 'nsis,msi') }
    elseif ($Mode -eq 'Msi') { $tauriArgs += @('--bundles', 'msi') }
    else { $tauriArgs += '--no-bundle' }
    if ($signingCertificate -and $Mode -ne 'Msix') {
        $signingConfig = Join-Path $srcTauri 'gen\signing-config.json'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $signingConfig) | Out-Null
        $signingJson = @{ bundle = @{ windows = @{ certificateThumbprint = $signingCertificate.Thumbprint; digestAlgorithm = 'sha256';
            signCommand = @{ cmd = (Get-WindowsSdkTool -Name signtool.exe); args = @('sign', '/fd', 'SHA256', '/sha1', $signingCertificate.Thumbprint, '%1') } } } } | ConvertTo-Json -Depth 6
        [IO.File]::WriteAllText($signingConfig, $signingJson, (New-Object Text.UTF8Encoding($false)))
        $tauriArgs += @('--config', $signingConfig)
    }
    $tauriArgs += @('--', '--locked')
    Write-Step "npm $($tauriArgs -join ' ')"
    Push-Location $desktop
    try {
        Invoke-Checked npm.cmd @tauriArgs
    } finally {
        Pop-Location
    }

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
    $distAssets = Get-Content -Raw -LiteralPath (Join-Path $desktop 'dist\library\assets.json')
    foreach ($claim in 'fixture_tone', 'reference_only') {
        if ($distAssets -match [regex]::Escape("`"$claim`"")) { throw "dist/library/assets.json carries a dev fixture ($claim); release must not ship spec-fixture, cube-fixture or bench-ref" }
    }
    $exampleChunk = @(Get-ChildItem -LiteralPath (Join-Path $desktop 'dist\assets') -Filter 'viewport.example-*.js' -ErrorAction SilentlyContinue)
    if ($exampleChunk.Count -gt 0) { throw "dist has $($exampleChunk[0].Name); VITE_GEN_AUDIO_FIXTURES leaked into a release build" }
    Write-Step "frontend dist ok $($index.Path) sha256 $($index.Sha256) ($($index.State); public files match; no dev fixtures)"

    $outputs = @(Get-ChildItem -Path (Join-Path $releaseDir 'build') -Filter 'output' -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -like 'gen-audio-desktop-*' })
    foreach ($output in $outputs) {
        if (Select-String -LiteralPath $output.FullName -Pattern 'cargo:rustc-cfg=dev' -SimpleMatch -Quiet) {
            throw "dev build detected ($($output.FullName) has cargo:rustc-cfg=dev); this exe would load http://localhost:1420"
        }
    }
    Write-Step "no dev cfg in gen-audio-desktop build-script output ($($outputs.Count) checked)"
    $fingerprints = @(Get-ChildItem -Path (Join-Path $releaseDir '.fingerprint') -Directory -Filter 'gen-audio-desktop-*' -ErrorAction SilentlyContinue |
            Where-Object { @(Get-ChildItem -LiteralPath $_.FullName -Filter 'lib-gen_audio_desktop_lib*' -ErrorAction SilentlyContinue).Count -gt 0 })
    Write-Step "gen-audio-desktop lib fingerprints: $($fingerprints.Count) ($(($fingerprints | ForEach-Object { $_.Name }) -join ', '))"
    if ($fingerprints.Count -ne 1) {
        throw "expected exactly one gen-audio-desktop lib fingerprint under $releaseDir\.fingerprint, found $($fingerprints.Count)"
    }

    $exe = Assert-BuildOutput -LiteralPath $exePath -What 'release gen-audio.exe' -Before $before.exe
    Write-Step "exe $($exe.Path) sha256 $($exe.Sha256) ($($exe.State))"
    if ($Mode -in @('Full', 'Msi')) {
        $bundleDir = Join-Path $releaseDir 'bundle'
        $bundleKinds = if ($Mode -eq 'Msi') { @('msi') } else { @('nsis', 'msi') }
        foreach ($kind in $bundleKinds) {
            $found = @(Get-ChildItem -Path (Join-Path $bundleDir $kind) -Filter $installerGlob[$kind] -File -ErrorAction SilentlyContinue)
            if ($found.Count -ne 1) { throw "expected one $kind installer $($installerGlob[$kind]) (version $($conf.version)) under $bundleDir\$kind, found $($found.Count): $(($found | ForEach-Object { $_.Name }) -join ', ')" }
            $installer = $found[0].FullName
            if ($signingCertificate) { Invoke-Checked (Get-WindowsSdkTool -Name signtool.exe) verify /pa /v $installer }
            $built = Assert-BuildOutput -LiteralPath $installer -What "$kind installer" -Before $before[$installer]
            Write-Output "INSTALLER path=$($built.Path) size=$($built.Length) sha256=$($built.Sha256) state=$($built.State)"
        }
    }
    if ($Mode -eq 'Msix') {
        $msix = New-GenAudioMsix -Executable $exe.Path -Sidecar $sidecar.FullName -Version $conf.version -Architecture $architecture -Icon (Join-Path $srcTauri 'icons\128x128@2x.png') -OutputDirectory (Join-Path $releaseDir 'bundle\msix') -Certificate $signingCertificate -RuntimePayload $runtimePayload
        Write-Output "INSTALLER path=$($msix.Path) sha256=$($msix.Sha256) publisher=$($msix.Publisher) certificate=$($msix.CertificateThumbprint)"
        $msix | ConvertTo-Json | Set-Content -LiteralPath (Join-Path (Split-Path -Parent $msix.Path) 'build-receipt.json') -Encoding UTF8
        if ($Install) {
            $installed = Install-GenAudioMsix -Package $msix
            Write-Output "INSTALL_OK package=$($installed.PackageFullName) location=$($installed.InstallLocation) exe_sha256=$($msix.ExeSha256) sidecar_sha256=$($msix.SidecarSha256)"
        }
    }
    Write-Output "BUILD_OK mode=$Mode exe=$($exe.Path) size=$($exe.Length) sha256=$($exe.Sha256) state=$($exe.State) log=$buildLog"
} catch {
    Write-Output "BUILD_FAIL log=$buildLog $($_.Exception.Message)"
    throw
} finally {
    Stop-Transcript | Out-Null
    $env:CARGO_TARGET_DIR = $savedTargetDir
    $env:CARGO_BUILD_JOBS = $savedBuildJobs
    $env:VITE_GEN_AUDIO_FIXTURES = $savedFixtures
    $env:CARGO_INCREMENTAL = $savedIncremental
    $env:RUSTC_WRAPPER = $savedRustWrapper
    $env:RUSTC_WORKSPACE_WRAPPER = $savedWorkspaceWrapper
    $env:TAURI_WINDOWS_SIGNTOOL_PATH = $savedSignToolPath
    if ($freshNpmCache -and (Test-Path -LiteralPath $freshNpmCache)) {
        $resolvedCache = (Resolve-Path -LiteralPath $freshNpmCache).Path
        $cacheBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if ($resolvedCache -ne $freshNpmCache -or -not $resolvedCache.StartsWith($cacheBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup of an unexpected npm cache directory' }
        Remove-Item -LiteralPath $resolvedCache -Recurse -Force
    }
    Pop-Location
}
