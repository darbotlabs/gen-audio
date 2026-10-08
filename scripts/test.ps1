<#
.SYNOPSIS
Run every Gen-Audio check from one entry point and exit non-zero on any failure.

.DESCRIPTION
Steps, in order (skip any with -Skip):
  Pssa    PSScriptAnalyzer over scripts/ with ./PSScriptAnalyzerSettings.psd1; any finding fails.
  Sprawl  the script-sprawl gate (see Invoke-SprawlGate below).
  Cargo   cargo test --workspace (Windows; elsewhere the Tauri crate is excluded,
          the same as the Linux CI job, because it needs the GTK/WebKit libs).
  Npm     apps/desktop: npm test, then tsc --noEmit.
  Python  pytest over tests/ (covers src/gen_audio and the scripts/*.py shims).

The sprawl gate scans the working tree on disk, including untracked and
gitignored files (artifacts/ counts), because one-off scripts hide in ignored
folders. -SprawlExclude takes paths relative to the repo root to leave out of
that scan; every excluded path is printed so the run says what it skipped.

Logs: artifacts/test-logs/<yyyyMMdd-HHmmss>-<Tag>/<step>.log
Works on Windows PowerShell 5.1 and PowerShell 7.

.EXAMPLE
pwsh -NoProfile -File scripts/test.ps1 -Tag pr5
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test.ps1 -Skip Cargo,Npm,Python
#>
[CmdletBinding()]
param(
    [string]$Tag = 'local',
    # Pssa, Sprawl, Cargo, Npm, Python. Comma-separated also works with -File.
    [string[]]$Skip = @(),
    [string[]]$SprawlExclude = @(),
    [string]$Python = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
# Script-scope copies; the step bodies below read these.
$script:skipSteps = @($Skip | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
foreach ($name in $script:skipSteps) {
    if (@('Pssa', 'Sprawl', 'Cargo', 'Npm', 'Python') -notcontains $name) { throw "-Skip $name is not a step (Pssa, Sprawl, Cargo, Npm, Python)" }
}
$script:sprawlExcludes = @($SprawlExclude | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$script:pythonExe = $Python
$logDir = Join-Path $root ("artifacts\test-logs\{0}-{1}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), ($Tag -replace '[^A-Za-z0-9_.-]', '_'))
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$results = New-Object System.Collections.Generic.List[object]

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

function Invoke-Step {
    param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][scriptblock]$Body)
    if ($script:skipSteps -contains $Name) {
        $results.Add([pscustomobject]@{ Step = $Name; Result = 'SKIP'; Seconds = 0; Detail = '-Skip' })
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
    $py = $script:pythonExe
    if (-not $py) { $py = $env:GEN_AUDIO_PYTHON }
    if (-not $py) { $py = $(if (Test-IsWindowsHost) { 'python' } else { 'python3' }) }
    $sep = [System.IO.Path]::PathSeparator
    $saved = $env:PYTHONPATH
    $env:PYTHONPATH = (Join-Path $root 'src') + $(if ($saved) { "$sep$saved" } else { '' })
    try {
        $code = Invoke-Logged -Log $log -File $py -Arguments @('-m', 'pytest', 'tests')
    } finally {
        $env:PYTHONPATH = $saved
    }
    if ($code -ne 0) { throw "pytest exit $code" }
    $summary = Select-String -LiteralPath $log -Pattern '\d+ passed' | Select-Object -Last 1
    "pytest: $(if ($summary) { $summary.Line.Trim() })"
}

$failed = @($results | Where-Object { $_.Result -eq 'FAIL' })
$results | Format-Table -AutoSize | Out-String -Width 220 | Out-Host
Write-Output ("TEST_SUMMARY {0} pass={1} fail={2} skip={3} logs={4}" -f $(if ($failed.Count) { 'FAIL' } else { 'PASS' }),
    @($results | Where-Object { $_.Result -eq 'PASS' }).Count, $failed.Count, @($results | Where-Object { $_.Result -eq 'SKIP' }).Count, $logDir)
if ($failed.Count -gt 0) { exit 1 }
exit 0
