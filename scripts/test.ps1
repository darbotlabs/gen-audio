<#
.SYNOPSIS
Run every Gen-Audio check from one entry point and exit non-zero on any failure.

.DESCRIPTION
Preflight (always first, cannot be skipped): git ls-files --eol. A tracked
file whose .gitattributes rule says eol=lf but whose working copy has CRLF
(or mixed) line endings fails the run before any step: byte-equality tests
and media sha256 would otherwise fail later for a checkout reason. It lists
the files and prints the fix; it never rewrites anything itself.

Steps, in order (skip any with -Skip):
  Wavs    -WithWav only: every WAV in schemas/asset-object/media.lock.json is
          staged in apps/desktop/public/library and its sha256 equals the lock.
  Pssa    PSScriptAnalyzer over scripts/ with ./PSScriptAnalyzerSettings.psd1; any finding fails.
  Sprawl  the script-sprawl gate (see Invoke-SprawlGate below).
  Regen   rerun the committed-output generators and fail on any drift:
          cargo run -p gen-audio-core --example build_assets   (assets.json,
            assets.dev.json, fixtures_v1.json, viewport.example/release.json,
            media.lock.json)
          cargo run -p gen-audio-core --example asset_vectors  (v1.json)
          python scripts/cube_revision.py manifest             (manifest.json
            cube mirror, from the cube JSON)
          then git diff --exit-code over those files. build_assets hashes the
          gitignored library WAVs into clip uids: a missing WAV FAILS the step
          (never a skip). -FromLock (CI, which has no WAVs) takes their facts
          from schemas/asset-object/media.lock.json instead and still checks
          each cube JSON's source_sha256 against them.
          PNGs (*_spec2d.png, cube PNGs) are not byte-diffed: CPython on
          Windows ships zlib-ng, so the same pixels compress to different bytes.
          tests/test_spectrogram_strip.py regenerates each strip and compares
          decoded pixels, and tests/test_cube_layers.py regenerates all five
          rev 3 cube JSON files byte for byte (both need the WAVs).
  Cargo   cargo test --workspace (Windows; elsewhere the Tauri crate is excluded,
          the same as the Linux CI job, because it needs the GTK/WebKit libs).
  Npm     apps/desktop: npm test, then tsc --noEmit.
  Python  pytest over tests/ (covers src/gen_audio and the scripts/*.py shims).

Modes:
  default   Regen needs the library WAVs on disk; a missing one FAILS Regen.
  -FromLock CI, no WAVs: Regen takes WAV facts from media.lock.json. It ties
            each cube JSON to its WAV sha256 but cannot see a hand-edited cube.
  -WithWav  the merge check (SMAX before review, Optimus before stamping; the
            PR template asks for it). Adds the Wavs step, runs Regen in the
            default mode, and sets GEN_AUDIO_REQUIRE_WAVS=1 so pytest FAILS
            any test that would skip for a missing WAV (tests/conftest.py):
            the byte-for-byte cube regeneration, inv_hdr and strip tests
            must run. Stage the WAVs first (from SMAX D:\gen-audio\artifacts\library
            or another verified copy); the Wavs step proves they are the
            locked bytes.

The sprawl gate scans the working tree on disk, including untracked and
gitignored files (artifacts/ counts), because one-off scripts hide in ignored
folders. -SprawlExclude takes paths relative to the repo root to leave out of
that scan; every excluded path is printed so the run says what it skipped.

Logs: artifacts/test-logs/<yyyyMMdd-HHmmss>-<Tag>/<step>.log
Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
pwsh -NoProfile -File scripts/test.ps1 -Tag pr5
pwsh -NoProfile -File scripts/test.ps1 -Tag ci -FromLock -Skip Cargo,Npm,Python
pwsh -NoProfile -File scripts/test.ps1 -Tag merge-check -WithWav
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test.ps1 -Skip Cargo,Npm,Python
#>
[CmdletBinding()]
param(
    [string]$Tag = 'local',
    # Pssa, Sprawl, Regen, Cargo, Npm, Python. Comma-separated also works with -File.
    [string[]]$Skip = @(),
    [string[]]$SprawlExclude = @(),
    [string]$Python = '',
    # CI only: library WAV identity from media.lock.json (build_assets --from-lock).
    [switch]$FromLock,
    # Merge check: staged WAVs must match media.lock.json and no WAV test may skip.
    [switch]$WithWav
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
# Script-scope copies; the step bodies below read these.
$script:skipSteps = @($Skip | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
foreach ($name in $script:skipSteps) {
    if (@('Pssa', 'Sprawl', 'Regen', 'Cargo', 'Npm', 'Python') -notcontains $name) { throw "-Skip $name is not a step (Pssa, Sprawl, Regen, Cargo, Npm, Python)" }
}
$script:sprawlExcludes = @($SprawlExclude | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$script:pythonExe = $Python
$script:fromLock = [bool]$FromLock
$script:withWav = [bool]$WithWav
if ($script:fromLock -and $script:withWav) { throw '-FromLock and -WithWav exclude each other: -WithWav needs the staged WAVs, -FromLock is for runs without them' }
$logDir = Join-Path $root ("artifacts\test-logs\{0}-{1}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), ($Tag -replace '[^A-Za-z0-9_.-]', '_'))
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$results = New-Object System.Collections.Generic.List[object]
$script:skipDetail = '-Skip'

function Add-LogLine {
    # Append lines to a log as UTF-8 and pass them on. (Tee-Object -Append
    # writes UTF-16 on Windows PowerShell 5.1, which breaks Select-String.)
    param([Parameter(Mandatory = $true)][string]$Log, [Parameter(ValueFromPipeline = $true)]$InputObject)
    process {
        # Native stderr arrives as ErrorRecords under 5.1; keep the text, not "RemoteException".
        $line = if ($InputObject -is [System.Management.Automation.ErrorRecord] -and $InputObject.TargetObject -is [string]) { $InputObject.TargetObject } else { "$InputObject" }
        [System.IO.File]::AppendAllText($Log, $line + [Environment]::NewLine, (New-Object System.Text.UTF8Encoding($false)))
        $line
    }
}

function Invoke-Logged {
    # Run a native command, tee its output to a log, return the exit code.
    param([Parameter(Mandatory = $true)][string]$Log, [Parameter(Mandatory = $true)][string]$File, [string[]]$Arguments = @())
    "> $File $($Arguments -join ' ')" | Add-LogLine -Log $Log | Out-Null
    $ErrorActionPreference = 'Continue'
    & $File @Arguments 2>&1 | Add-LogLine -Log $Log | Out-Host
    return $LASTEXITCODE
}

function Get-EolMismatch {
    # `git ls-files --eol` rows ("i/lf  w/crlf  attr/text eol=lf<TAB>path")
    # whose attribute says eol=lf while the working copy is CRLF or mixed.
    # Typical cause: core.autocrlf=true and a checkout made before
    # .gitattributes existed. Emits objects with Path and Index (lf/crlf/...).
    param([Parameter(Mandatory = $true)][string]$Root)
    $rows = @(& git -C $Root -c core.quotepath=off ls-files --eol 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "git ls-files --eol failed: $($rows -join ' ')" }
    foreach ($row in $rows) {
        $parts = "$row" -split "`t", 2
        if ($parts.Count -ne 2) { continue }
        $info = $parts[0]
        if ($info -match '\bw/(crlf|mixed)\b' -and $info -match 'attr/.*\beol=lf\b') {
            $index = if ($info -match '\bi/(\S+)') { $Matches[1] } else { '' }
            [pscustomobject]@{ Path = $parts[1]; Index = $index }
        }
    }
}

function Get-PythonExe {
    # -Python, then GEN_AUDIO_PYTHON, then python / python3 on PATH.
    if ($script:pythonExe) { return $script:pythonExe }
    if ($env:GEN_AUDIO_PYTHON) { return $env:GEN_AUDIO_PYTHON }
    if (Test-IsWindowsHost) { return 'python' }
    return 'python3'
}

function Invoke-Step {
    param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][scriptblock]$Body)
    if ($script:skipSteps -contains $Name) {
        $results.Add([pscustomobject]@{ Step = $Name; Result = 'SKIP'; Seconds = 0; Detail = $script:skipDetail })
        return
    }
    Write-Step "== $Name"
    $log = Join-Path $logDir "$Name.log"
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $detail = ''
    try {
        $detail = [string](& $Body $log)
        $result = 'PASS'
    } catch {
        $result = 'FAIL'
        $detail = $_.Exception.Message
        $detail | Add-LogLine -Log $log | Out-Null
    }
    $results.Add([pscustomobject]@{ Step = $Name; Result = $result; Seconds = [int]$watch.Elapsed.TotalSeconds; Detail = $detail })
    Write-Step "== $Name $result $detail"
}

function Get-ScriptFile {
    # Walk the tree on disk (tracked, untracked and ignored files alike).
    # Prunes dependency and build caches that never hold authored scripts:
    # .git, node_modules, target, Python caches, virtualenvs (any folder with
    # pyvenv.cfg), and generated app output (apps/desktop/dist, src-tauri/gen).
    param([Parameter(Mandatory = $true)][string]$Root, [string[]]$Exclude = @())
    $pruneNames = @('.git', 'node_modules', 'target', '__pycache__', '.pytest_cache', '.mypy_cache', '.ruff_cache', '.venv', 'venv')
    $prunePaths = @('apps/desktop/dist', 'apps/desktop/src-tauri/gen') + @($Exclude | ForEach-Object { ($_ -replace '\\', '/').Trim('/') })
    $extensions = @('.ps1', '.psm1', '.bat', '.cmd', '.py')
    $stack = New-Object System.Collections.Generic.Stack[string]
    $stack.Push($Root)
    while ($stack.Count -gt 0) {
        $dir = $stack.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue)) {
            $rel = $entry.FullName.Substring($Root.Length).TrimStart('\', '/') -replace '\\', '/'
            if ($entry.PSIsContainer) {
                if ($pruneNames -contains $entry.Name -or $entry.Name -like '*.egg-info' -or $prunePaths -contains $rel) { continue }
                if (Test-Path -LiteralPath (Join-Path $entry.FullName 'pyvenv.cfg')) { continue }
                $stack.Push($entry.FullName)
            } elseif ($extensions -contains $entry.Extension.ToLowerInvariant() -or ($entry.Name -eq 'manifest.json' -and $rel -match '(^|/)library/manifest\.json$')) {
                [pscustomobject]@{ Rel = $rel; Full = $entry.FullName }
            }
        }
    }
}

function Invoke-SprawlGate {
    <#
    Fails on any finding:
      outside-allowlist  a *.ps1/*.psm1/*.bat/*.cmd/*.py outside the allowlist:
                         scripts/{build-tauri-windows,test,mcp-call,ui-shot}.ps1,
                         scripts/lib/*.ps1, scripts/*.py, tests/**/*.py, src/**/*.py
      thick-shim         a scripts/*.py that is not a thin CLI shim into gen_audio.cli
                         (over 20 lines or no `from gen_audio.cli` import)
      vcvars             a script other than scripts/lib/devenv.ps1 sources vcvars/VsDevCmd
      tauri-build        more than one script calls the tauri CLI build
      bare-desktop-cargo any script compiles the desktop crate with cargo directly
      cargo-clean        any script runs cargo clean
      manifest-copy      a library/manifest.json other than apps/desktop/public/library/manifest.json
    This file defines the patterns, so it is not content-scanned itself.
    #>
    param([Parameter(Mandatory = $true)][string]$Root, [string[]]$Exclude = @())
    $entryPoints = @('scripts/build-tauri-windows.ps1', 'scripts/test.ps1', 'scripts/mcp-call.ps1', 'scripts/ui-shot.ps1')
    $self = 'scripts/test.ps1'
    $vcvarsOwner = 'scripts/lib/devenv.ps1'
    $tauriOwner = 'scripts/build-tauri-windows.ps1'
    $patterns = @{
        'vcvars'             = 'vcvars(64|32|all)?\.bat|VsDevCmd\.bat'
        'tauri-build'        = '@tauri-apps/cli|cargo[ -]tauri\s+build|\btauri(\.cmd)?[''"]?\s+build\b'
        'bare-desktop-cargo' = 'cargo\s+(build|run|rustc|install)\b[^\r\n]*(-p|--package)\s+gen-audio-desktop'
        'cargo-clean'        = 'cargo\s+clean\b'
    }
    $findings = New-Object System.Collections.Generic.List[string]
    $tauriCallers = New-Object System.Collections.Generic.List[string]
    foreach ($exclude in $Exclude) { Write-Output "SPRAWL_EXCLUDED $exclude" }
    $files = @(Get-ScriptFile -Root $Root -Exclude $Exclude)
    foreach ($file in $files) {
        $rel = $file.Rel
        if ($file.Full -like '*manifest.json') {
            if ($rel -ne 'apps/desktop/public/library/manifest.json') { $findings.Add("manifest-copy $rel") }
            continue
        }
        $allowed = ($entryPoints -contains $rel) -or ($rel -match '^scripts/lib/[^/]+\.ps1$') -or
            ($rel -match '^scripts/[^/]+\.py$') -or ($rel -match '^tests/.+\.py$') -or ($rel -match '^src/.+\.py$')
        if (-not $allowed) { $findings.Add("outside-allowlist $rel") }
        if ($rel -eq $self) { continue }
        $text = [System.IO.File]::ReadAllText($file.Full)
        if ($rel -match '^scripts/[^/]+\.py$') {
            $lineCount = @($text -split "`n").Count
            if ($lineCount -gt 20 -or $text -notmatch 'from gen_audio\.cli') { $findings.Add("thick-shim $rel ($lineCount lines)") }
        }
        if ($text -match $patterns['vcvars'] -and $rel -ne $vcvarsOwner) { $findings.Add("vcvars $rel") }
        if ($text -match $patterns['tauri-build']) { $tauriCallers.Add($rel) }
        if ($text -match $patterns['bare-desktop-cargo']) { $findings.Add("bare-desktop-cargo $rel") }
        if ($text -match $patterns['cargo-clean']) { $findings.Add("cargo-clean $rel") }
    }
    foreach ($caller in $tauriCallers) {
        if ($tauriCallers.Count -gt 1 -or $caller -ne $tauriOwner) { $findings.Add("tauri-build $caller") }
    }
    foreach ($finding in $findings) { Write-Output "SPRAWL $finding" }
    Write-Output ("SPRAWL_GATE {0} files={1} findings={2}" -f $(if ($findings.Count) { 'FAIL' } else { 'PASS' }), $files.Count, $findings.Count)
    return $findings.Count
}

Write-Step "scripts/test.ps1 -Tag $Tag (PowerShell $($PSVersionTable.PSVersion)); logs in $logDir"
if (Test-IsWindowsHost) { Import-VsDevEnv }

Invoke-Step 'Eol' {
    param($log)
    $bad = @(Get-EolMismatch -Root $root)
    if ($bad.Count -eq 0) { return 'git ls-files --eol: no CRLF working copies under an eol=lf rule' }
    $lines = @("EOL_PREFLIGHT FAIL: $($bad.Count) tracked file(s) have eol=lf in .gitattributes but CRLF in the working copy:")
    $lines += @($bad | ForEach-Object { "  EOL_CRLF $($_.Path) (index $($_.Index))" })
    $lines += 'Fix it yourself (this check never rewrites files). Commit or stash your changes first.'
    if (@($bad | Where-Object { $_.Index -ne 'lf' }).Count -gt 0) {
        $lines += '  The index holds CRLF too: git add --renormalize . ; then commit the result.'
    }
    if (@($bad | Where-Object { $_.Index -eq 'lf' }).Count -gt 0) {
        $lines += '  The index is already LF, only the working copy is CRLF: refresh the checkout with'
        $lines += '    git rm --cached -r . ; git reset --hard'
        $lines += '  WARNING: reset --hard discards uncommitted changes. Commit or stash first.'
    }
    $lines | Add-LogLine -Log $log | Out-Host
    throw "$($bad.Count) CRLF working file(s) under eol=lf: $(@($bad | ForEach-Object { $_.Path }) -join ', ')"
}
if (@($results | Where-Object { $_.Step -eq 'Eol' -and $_.Result -eq 'FAIL' }).Count -gt 0) {
    # Every later step would fail or mislead on these bytes; stop here.
    $script:skipSteps = @('Wavs', 'Pssa', 'Sprawl', 'Regen', 'Cargo', 'Npm', 'Python')
    $script:skipDetail = 'Eol preflight failed'
}

if ($script:withWav) {
    Invoke-Step 'Wavs' {
        param($log)
        $library = Join-Path $root 'apps\desktop\public\library'
        $lock = Get-Content -LiteralPath (Join-Path $root 'schemas\asset-object\media.lock.json') -Raw | ConvertFrom-Json
        $names = @($lock.media.PSObject.Properties | ForEach-Object { $_.Name })
        if ($names.Count -eq 0) { throw 'media.lock.json lists no WAVs' }
        $bad = New-Object System.Collections.Generic.List[string]
        foreach ($name in $names) {
            $want = [string]$lock.media.$name.sha256
            $path = Join-Path $library $name
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
                $bad.Add("WAV_MISSING $name (stage it in apps/desktop/public/library)")
                continue
            }
            $got = Get-Sha256 -LiteralPath $path
            $line = "WAV $name sha256 $got"
            if ($got -ne $want) { $bad.Add("WAV_MISMATCH $name sha256 $got, media.lock.json $want") } else { $line | Add-LogLine -Log $log | Out-Host }
        }
        if ($bad.Count -gt 0) {
            $bad | Add-LogLine -Log $log | Out-Host
            throw "$($bad.Count) of $($names.Count) locked WAV(s) missing or not the locked bytes: $(@($bad | ForEach-Object { ($_ -split ' ')[1] }) -join ', ')"
        }
        "$($names.Count)/$($names.Count) staged WAVs match media.lock.json sha256"
    }
}

Invoke-Step 'Pssa' {
    param($log)
    $module = Get-Module -ListAvailable -Name PSScriptAnalyzer | Where-Object { $_.Version -eq [version]'1.24.0' } | Select-Object -First 1
    if (-not $module) { throw 'PSScriptAnalyzer 1.24.0 is not installed. Install-Module PSScriptAnalyzer -RequiredVersion 1.24.0 -Scope CurrentUser' }
    Import-Module $module.Path -Force
    $settings = Join-Path $root 'PSScriptAnalyzerSettings.psd1'
    $found = @(Invoke-ScriptAnalyzer -Path (Join-Path $root 'scripts') -Recurse -Settings $settings) +
        @(Invoke-ScriptAnalyzer -Path $settings -Settings $settings)
    foreach ($item in $found) {
        $line = '{0}:{1} {2} {3} {4}' -f $item.ScriptName, $item.Line, $item.Severity, $item.RuleName, $item.Message
        $line | Add-LogLine -Log $log | Out-Host
    }
    "PSSA findings=$($found.Count)" | Add-LogLine -Log $log | Out-Null
    if ($found.Count -gt 0) { throw "PSScriptAnalyzer findings=$($found.Count)" }
    'findings=0'
}

Invoke-Step 'Sprawl' {
    param($log)
    $lines = @(Invoke-SprawlGate -Root $root -Exclude $script:sprawlExcludes)
    $count = [int]$lines[-1]
    $lines | Select-Object -SkipLast 1 | Add-LogLine -Log $log | Out-Host
    if ($count -gt 0) { throw "sprawl findings=$count" }
    'findings=0'
}

Invoke-Step 'Regen' {
    param($log)
    $library = 'apps/desktop/public/library'
    $generated = @("$library/assets.json", "$library/manifest.json", 'schemas/asset-object/fixtures/assets.dev.json',
        'schemas/asset-object/media.lock.json', 'schemas/asset-object/vectors/fixtures_v1.json',
        'schemas/asset-object/vectors/v1.json', 'schemas/examples/viewport.example.json', 'schemas/examples/viewport.release.json')
    # Start from the committed bytes, or the diff below would blame the generators for hand edits.
    $dirty = @(& git status --porcelain -- @generated)
    if ($LASTEXITCODE -ne 0) { throw 'git status failed' }
    if ($dirty.Count -gt 0) { throw "generated files already differ from HEAD (commit or restore them first): $($dirty -join '; ')" }
    # build_assets names any missing WAV and exits 1; there is no skip path.
    $buildArgs = @('run', '-q', '-p', 'gen-audio-core', '--example', 'build_assets')
    if ($script:fromLock) { $buildArgs += @('--', '--from-lock') }
    if ((Invoke-Logged -Log $log -File 'cargo' -Arguments $buildArgs) -ne 0) {
        throw "build_assets failed$(if (-not $script:fromLock) { ' (a missing library WAV fails here; stage the WAVs, or -FromLock in CI)' })"
    }
    if ((Invoke-Logged -Log $log -File 'cargo' -Arguments @('run', '-q', '-p', 'gen-audio-core', '--example', 'asset_vectors')) -ne 0) { throw 'asset_vectors failed' }
    if ((Invoke-Logged -Log $log -File (Get-PythonExe) -Arguments @('scripts/cube_revision.py', 'manifest')) -ne 0) { throw 'cube_revision.py manifest failed' }
    $code = Invoke-Logged -Log $log -File 'git' -Arguments (@('diff', '--exit-code', '--stat', '--') + $generated)
    if ($code -ne 0) { throw "generated files drifted after build_assets + asset_vectors + manifest sync; see git diff (exit $code)" }
    "build_assets$(if ($script:fromLock) { ' --from-lock' } else { ' (WAVs on disk)' }) + asset_vectors + manifest sync reproduce the committed files"
}

Invoke-Step 'Cargo' {
    param($log)
    $cargoArgs = @('test', '--workspace')
    if (Test-IsWindowsHost) {
        # tauri-build checks that the externalBin sidecar exists; stage a debug one if none is there.
        $triple = ((& rustc -vV) | Select-String -Pattern '^host: ').ToString().Split(':', 2)[1].Trim()
        $staged = Join-Path $root "apps\desktop\src-tauri\binaries\gen-audio-mcp-$triple.exe"
        if (-not (Test-Path -LiteralPath $staged)) {
            if ((Invoke-Logged -Log $log -File 'cargo' -Arguments @('build', '-p', 'gen-audio-mcp')) -ne 0) { throw 'cargo build -p gen-audio-mcp failed' }
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $staged) | Out-Null
            Copy-Item -LiteralPath (Join-Path $root 'target\debug\gen-audio-mcp.exe') -Destination $staged
        }
    } else {
        $cargoArgs += @('--exclude', 'gen-audio-desktop')
    }
    $code = Invoke-Logged -Log $log -File 'cargo' -Arguments $cargoArgs
    if ($code -ne 0) { throw "cargo $($cargoArgs -join ' ') exit $code" }
    $passed = 0
    foreach ($match in (Select-String -LiteralPath $log -Pattern 'test result: ok\. (\d+) passed')) { $passed += [int]$match.Matches[0].Groups[1].Value }
    "cargo $($cargoArgs -join ' '): $passed passed"
}

Invoke-Step 'Npm' {
    param($log)
    Push-Location (Join-Path $root 'apps\desktop')
    try {
        if (-not (Test-Path -LiteralPath 'node_modules')) {
            if ((Invoke-Logged -Log $log -File 'npm' -Arguments @('ci')) -ne 0) { throw 'npm ci failed' }
        }
        $code = Invoke-Logged -Log $log -File 'npm' -Arguments @('test')
        if ($code -ne 0) { throw "npm test exit $code" }
        $code = Invoke-Logged -Log $log -File 'npx' -Arguments @('--no-install', 'tsc', '--noEmit', '-p', '.')
        if ($code -ne 0) { throw "tsc --noEmit exit $code" }
    } finally {
        Pop-Location
    }
    $pass = Select-String -LiteralPath $log -Pattern '^# pass (\d+)' | Select-Object -Last 1
    "npm test + tsc --noEmit ok$(if ($pass) { ', ' + $pass.Matches[0].Value.TrimStart('# ') })"
}

Invoke-Step 'Python' {
    param($log)
    $py = Get-PythonExe
    $sep = [System.IO.Path]::PathSeparator
    $saved = $env:PYTHONPATH
    $env:PYTHONPATH = (Join-Path $root 'src') + $(if ($saved) { "$sep$saved" } else { '' })
    $savedRequire = $env:GEN_AUDIO_REQUIRE_WAVS
    if ($script:withWav) { $env:GEN_AUDIO_REQUIRE_WAVS = '1' }
    try {
        $code = Invoke-Logged -Log $log -File $py -Arguments @('-m', 'pytest', 'tests')
    } finally {
        $env:PYTHONPATH = $saved
        $env:GEN_AUDIO_REQUIRE_WAVS = $savedRequire
    }
    if ($code -ne 0) { throw "pytest exit $code" }
    $summary = Select-String -LiteralPath $log -Pattern '\d+ passed' | Select-Object -Last 1
    "pytest: $(if ($summary) { $summary.Line.Trim() })$(if ($script:withWav) { ' (GEN_AUDIO_REQUIRE_WAVS=1: no WAV skips)' })"
}

$failed = @($results | Where-Object { $_.Result -eq 'FAIL' })
$results | Format-Table -AutoSize | Out-String -Width 220 | Out-Host
Write-Output ("TEST_SUMMARY {0} pass={1} fail={2} skip={3} logs={4}" -f $(if ($failed.Count) { 'FAIL' } else { 'PASS' }),
    @($results | Where-Object { $_.Result -eq 'PASS' }).Count, $failed.Count, @($results | Where-Object { $_.Result -eq 'SKIP' }).Count, $logDir)
if ($failed.Count -gt 0) { exit 1 }
exit 0
