<#
.SYNOPSIS
Capture the Gen-Audio window to a PNG with Win32 PrintWindow.

.DESCRIPTION
PrintWindow only (PW_RENDERFULLCONTENT). No input injection, no focus change,
no screen scraping fallback. The window must be visible (not hidden to the
tray); drive the app state first with scripts/mcp-call.ps1.
Fails if the capture is blank. Prints `SHOT path=... sha256=... size=WxH`.
Works on Windows PowerShell 5.1 and PowerShell 7 (Windows only).

.EXAMPLE
pwsh -NoProfile -File scripts/ui-shot.ps1 -Out C:\shots\library.png
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$ProcessName = 'gen-audio'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'lib\devenv.ps1')

if (-not (Test-IsWindowsHost)) { throw 'ui-shot.ps1 uses Win32 PrintWindow; run it on Windows.' }

# P/Invoke declarations only. Bitmap work stays in PowerShell, because
# compiling System.Drawing code in Add-Type fails on PowerShell 7.
if (-not ('GenAudioShot.Native' -as [type])) {
    Add-Type -Namespace GenAudioShot -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT rect);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool PrintWindow(System.IntPtr hWnd, System.IntPtr hdc, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool IsWindowVisible(System.IntPtr hWnd);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool IsIconic(System.IntPtr hWnd);
'@
}
Add-Type -AssemblyName System.Drawing

$process = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne [IntPtr]::Zero }) | Select-Object -First 1
if (-not $process) { throw "no visible window for process '$ProcessName' (not running, or hidden to the tray)" }
$hwnd = $process.MainWindowHandle
if (-not [GenAudioShot.Native]::IsWindowVisible($hwnd)) { throw "window of $ProcessName ($($process.Id)) is not visible" }
if ([GenAudioShot.Native]::IsIconic($hwnd)) { throw "window of $ProcessName ($($process.Id)) is minimized" }

$rect = New-Object GenAudioShot.Native+RECT
if (-not [GenAudioShot.Native]::GetWindowRect($hwnd, [ref]$rect)) { throw 'GetWindowRect failed' }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { throw "window has no area (${width}x${height})" }

$outPath = [System.IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outPath) | Out-Null
$bitmap = New-Object System.Drawing.Bitmap $width, $height
try {
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $hdc = $graphics.GetHdc()
        try {
            $ok = [GenAudioShot.Native]::PrintWindow($hwnd, $hdc, 2)
        } finally {
            $graphics.ReleaseHdc($hdc)
        }
    } finally {
        $graphics.Dispose()
    }
    if (-not $ok) { throw 'PrintWindow returned false' }
    # Blank check: sample a grid; an all-one-color capture is a failed render.
    $first = $bitmap.GetPixel(0, 0).ToArgb()
    $distinct = $false
    foreach ($y in 0..15) {
        foreach ($x in 0..15) {
            $px = $bitmap.GetPixel([int](($width - 1) * $x / 15), [int](($height - 1) * $y / 15)).ToArgb()
            if ($px -ne $first) { $distinct = $true; break }
        }
        if ($distinct) { break }
    }
    if (-not $distinct) { throw 'capture is a single color; the window did not render into PrintWindow' }
    $bitmap.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
    $bitmap.Dispose()
}
$hash = Get-Sha256 -LiteralPath $outPath
Write-Output "SHOT path=$outPath sha256=$hash size=${width}x${height} pid=$($process.Id)"
