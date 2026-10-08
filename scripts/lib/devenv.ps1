# Shared helpers for the canonical scripts. Dot-source it; it is not an entry point.
#   . (Join-Path $PSScriptRoot 'lib\devenv.ps1')
# Works on Windows PowerShell 5.1 and PowerShell 7.
#
# This is the only file allowed to import the MSVC environment (vcvars64).
# The sprawl gate in scripts/test.ps1 fails if any other script does.

Set-StrictMode -Version 2.0

function Write-Step {
    # Console progress for a person watching the run. Results that other
    # tools read (BUILD_OK, TEST_SUMMARY, sha256 lines) go to the pipeline.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'Progress text for the console only; never parsed.')]
    [CmdletBinding()]
    param([Parameter(Mandatory = $true, Position = 0)][string]$Message)
    Write-Host "[$((Get-Date).ToString('HH:mm:ss'))] $Message"
}

function Invoke-Checked {
    # Run a native command and throw on a non-zero exit code.
    # A simple function on purpose: it forwards $args untouched. An advanced
    # function (param block) binds common parameters, so on Windows
    # PowerShell 5.1 `cargo build -p x` lost `-p` to -PipelineVariable and
    # on PowerShell 7.4+ it threw. Fix from Darbot's PR #3.
    $file = [string]$args[0]
    $commandArgs = [string[]]@($args | Select-Object -Skip 1)
    # Native stderr (cargo progress) must not become a terminating error
    # when a caller redirects it under $ErrorActionPreference = 'Stop'.
    $ErrorActionPreference = 'Continue'
    # Through the host (stderr as plain text), so Start-Transcript records it:
    # Windows PowerShell 5.1 does not transcribe native output written
    # straight to the console.
    & $file @commandArgs 2>&1 | ForEach-Object { "$_" } | Out-Host
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        throw "$file $($commandArgs -join ' ') failed with exit $code"
    }
}

function Get-Sha256 {
    # Lower-case hex SHA-256 of a file, straight from .NET. Get-FileHash is a
    # script function in Windows PowerShell 5.1's Utility module, and it goes
    # missing when 5.1 inherits PowerShell 7's PSModulePath (Start-Process
    # from pwsh 7 does that), so the canonical scripts do not depend on it.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory = $true)][string]$LiteralPath)
    $full = (Resolve-Path -LiteralPath $LiteralPath).ProviderPath
    $stream = [System.IO.File]::Open($full, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $sha.ComputeHash($stream)
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
    return (($bytes | ForEach-Object { $_.ToString('x2') }) -join '')
}

function Get-LibraryWavProblem {
    # The gitignored library WAVs against schemas/asset-object/media.lock.json:
    # one line per WAV that is missing, has the wrong byte count, or the wrong
    # sha256 (WAV_MISSING / WAV_BYTES / WAV_MISMATCH). No output means all
    # match. Shared by build-tauri-windows.ps1 (a release must embed the real
    # WAVs) and test.ps1 -WithWav. A gate that passes by skipping is not a gate,
    # so an empty or unreadable lock is an error too.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory = $true)][string]$Root)
    $lockPath = Join-Path $Root 'schemas\asset-object\media.lock.json'
    if (-not (Test-Path -LiteralPath $lockPath -PathType Leaf)) { throw "media.lock.json is missing: $lockPath" }
    $lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
    $names = @(if ($lock.PSObject.Properties['media']) { $lock.media.PSObject.Properties | ForEach-Object { $_.Name } })
    if ($names.Count -eq 0) { throw "$lockPath lists no WAVs" }
    $library = Join-Path $Root 'apps\desktop\public\library'
    foreach ($name in $names) {
        $entry = $lock.media.$name
        $path = Join-Path $library $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            "WAV_MISSING $name (stage it in apps/desktop/public/library; *.wav is gitignored)"
            continue
        }
        $bytes = (Get-Item -LiteralPath $path).Length
        if ($entry.PSObject.Properties['bytes'] -and [int64]$entry.bytes -ne $bytes) {
            "WAV_BYTES $name has $bytes bytes, media.lock.json $($entry.bytes)"
            continue
        }
        $got = Get-Sha256 -LiteralPath $path
        if ($got -ne [string]$entry.sha256) { "WAV_MISMATCH $name sha256 $got, media.lock.json $($entry.sha256)" }
    }
}

function Select-CargoBinArtifact {
    # From `cargo build --message-format=json*` stdout lines, the
    # compiler-artifact cargo reported for binary $Bin: its executable path and
    # whether cargo's fingerprint said it was already up to date (fresh). That
    # is cargo's own verdict on staleness; file mtimes are not (a no-op
    # rebuild leaves the old mtime in place).
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][AllowEmptyString()][string[]]$Line, [Parameter(Mandatory = $true)][string]$Bin)
    $found = $null
    foreach ($text in $Line) {
        if (-not $text -or -not $text.StartsWith('{')) { continue }
        $message = $text | ConvertFrom-Json
        if (-not $message.PSObject.Properties['reason'] -or $message.reason -ne 'compiler-artifact') { continue }
        if ($message.target.name -ne $Bin -or @($message.target.kind) -notcontains 'bin') { continue }
        if (-not $message.PSObject.Properties['executable'] -or -not $message.executable) { continue }
        $found = [pscustomobject]@{ Executable = [string]$message.executable; Fresh = [bool]$message.fresh }
    }
    if (-not $found) { throw "cargo reported no compiler-artifact for bin $Bin, so there is no way to tell which binary it built" }
    return $found
}

function Invoke-CargoBinBuild {
    # cargo build one binary and return Select-CargoBinArtifact's answer
    # (Executable, Fresh). Throws on a non-zero exit. Diagnostics still go to
    # the console (json-render-diagnostics); stdout carries the JSON messages.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param([Parameter(Mandatory = $true)][string]$Package, [Parameter(Mandatory = $true)][string]$Bin, [string[]]$ExtraArgs = @())
    $cargoArgs = @('build', '-p', $Package, '--bin', $Bin, '--message-format=json-render-diagnostics') + $ExtraArgs
    $ErrorActionPreference = 'Continue'
    # stdout is cargo's JSON; stderr (diagnostics, progress) goes through the
    # host as text so a build transcript records it.
    $lines = New-Object System.Collections.Generic.List[string]
    & cargo @cargoArgs 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) { "$_" | Out-Host } else { $lines.Add([string]$_) }
    }
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "cargo $($cargoArgs -join ' ') failed with exit $code" }
    return Select-CargoBinArtifact -Line $lines.ToArray() -Bin $Bin
}

function Get-BuildLogPath {
    # Where a build-tauri-windows.ps1 run saves its transcript:
    # <Root>/target/logs/build-<12-char HEAD sha>-<yyyyMMdd-HHmmss>.log
    # ("nogit" in place of the sha outside a git checkout).
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory = $true)][string]$Root)
    $ErrorActionPreference = 'Continue'
    $sha = "$(& git -C $Root rev-parse --short=12 HEAD 2>$null)".Trim()
    if ($LASTEXITCODE -ne 0 -or -not $sha) { $sha = 'nogit' }
    $name = 'build-{0}-{1}.log' -f $sha, (Get-Date -Format 'yyyyMMdd-HHmmss')
    return (Join-Path (Join-Path (Join-Path $Root 'target') 'logs') $name)
}

function Test-IsWindowsHost {
    [CmdletBinding()]
    [OutputType([bool])]
    param()
    return ($env:OS -eq 'Windows_NT')
}

function Import-VsDevEnv {
    # Import the x64 MSVC build environment (cl.exe, link.exe, INCLUDE, LIB)
    # into this PowerShell session from vcvars64.bat.
    # Skips when cl.exe is already on PATH. Throws when Visual Studio or the
    # C++ build tools are missing; it never guesses a path.
    [CmdletBinding()]
    param()
    if (-not (Test-IsWindowsHost)) {
        Write-Verbose 'Import-VsDevEnv: not Windows, nothing to import.'
        return
    }
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
        Write-Step 'MSVC environment already present (cl.exe on PATH); skipping vcvars64'
        return
    }
    $installer = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $installer)) {
        throw "vswhere.exe not found at $installer. Install Visual Studio 2022 Build Tools with the 'Desktop development with C++' workload."
    }
    $vsPath = & $installer -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vsPath) {
        throw 'vswhere found no Visual Studio install with the x64 C++ tools (Microsoft.VisualStudio.Component.VC.Tools.x86.x64).'
    }
    $vcvars = Join-Path ([string]@($vsPath)[0]) 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) {
        throw "vcvars64.bat not found at $vcvars"
    }
    Write-Step "import MSVC environment from $vcvars"
    $lines = & cmd.exe /d /s /c "`"$vcvars`" >nul && set"
    if ($LASTEXITCODE -ne 0) {
        throw "vcvars64.bat failed with exit $LASTEXITCODE"
    }
    foreach ($line in $lines) {
        if ($line -match '^([^=]+)=(.*)$') {
            Set-Item -LiteralPath "Env:$($Matches[1])" -Value $Matches[2]
        }
    }
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        throw 'vcvars64.bat ran but cl.exe is still not on PATH.'
    }
}
