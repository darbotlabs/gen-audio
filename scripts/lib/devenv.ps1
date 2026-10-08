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
    & $file @commandArgs
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
