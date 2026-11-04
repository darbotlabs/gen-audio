<#
.SYNOPSIS
Run a checkout-local Gen-Audio service with one model and runtime layout.
.DESCRIPTION
Models live in models/, logs and generated files in artifacts/runtime/.
Runs in the foreground. -Check prints the launch plan without starting anything.
Desktop is the native release app; Preview is the Vite development server.
#>
[CmdletBinding()]
param(
    [ValidateSet('Mcp', 'Node', 'Desktop', 'Preview')]
    [string]$Service = 'Mcp',
    [ValidateRange(1, 65535)]
    [int]$Port,
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$root = Split-Path -Parent $PSScriptRoot
$models = Join-Path $root 'models'
$runtime = Join-Path $root 'artifacts\runtime'
$python = Join-Path $root '.venv\Scripts\python.exe'
$ports = @{ Mcp = 8765; Node = 8002; Desktop = 8765; Preview = 1420 }
if (-not $Port) { $Port = $ports[$Service] }
$environment = @{
    GEN_AUDIO_REPO = $root
    GEN_AUDIO_PYTHON = $python
    GEN_AUDIO_MODEL_DIR = $models
    GEN_AUDIO_KOKORO_MODEL = Join-Path $models 'kokoro-v1.0.onnx'
    GEN_AUDIO_KOKORO_VOICES = Join-Path $models 'voices-v1.0.bin'
    GENAID_MODEL_BASE = $models
    GEN_AUDIO_WORK_DIR = Join-Path $runtime 'mcp'
    GEN_AUDIO_MCP_ADDR = $(if ($Service -in @('Mcp', 'Desktop')) { "127.0.0.1:$Port" } else { '127.0.0.1:8765' })
}
$cwd = $root
$commandArgs = @()
switch ($Service) {
    'Mcp' {
        $exe = Join-Path $root 'target\release\gen-audio-mcp.exe'
        $commandArgs = @('--http', "127.0.0.1:$Port")
    }
    'Node' {
        $exe = $python
        $commandArgs = @('-c', "from gen_audio.node_http import serve_loopback; serve_loopback(port=$Port).serve_forever()")
    }
    'Desktop' { $exe = Join-Path $root 'target\release\gen-audio.exe' }
    'Preview' {
        $exe = 'npm.cmd'
        $cwd = Join-Path $root 'apps\desktop'
        $commandArgs = @('run', 'dev', '--', '--host', '127.0.0.1', '--port', "$Port", '--strictPort')
    }
}
$logName = $Service.ToLowerInvariant()
$outLog = Join-Path $runtime "$logName.out.log"
$errLog = Join-Path $runtime "$logName.err.log"
if ($Check) {
    [pscustomobject]@{
        service = $Service; executable = $exe; arguments = $commandArgs
        workingDirectory = $cwd; environment = $environment
        stdout = $outLog; stderr = $errLog
    } | ConvertTo-Json -Depth 5
    return
}
if ($Service -ne 'Preview') {
    foreach ($file in @($exe, $python, $environment.GEN_AUDIO_KOKORO_MODEL, $environment.GEN_AUDIO_KOKORO_VOICES)) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            throw "Missing runtime dependency: $file. See README.md, Checkout-local deployment."
        }
    }
}
New-Item -ItemType Directory -Force -Path $runtime, $environment.GEN_AUDIO_WORK_DIR | Out-Null
$saved = @{}
foreach ($key in $environment.Keys) {
    $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
    [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process')
}
Push-Location $cwd
try {
    Write-Output "Starting $Service on 127.0.0.1:$Port; logs: $runtime"
    & $exe @commandArgs 1>> $outLog 2>> $errLog
    if ($LASTEXITCODE -ne 0) { throw "$Service exited with code $LASTEXITCODE; see $errLog" }
} finally {
    Pop-Location
    foreach ($key in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process')
    }
}
