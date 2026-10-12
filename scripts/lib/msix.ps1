# MSIX support for the canonical Windows build entry point. Never exports private keys.
Set-StrictMode -Version 2.0

function Get-MsixPackageVersion {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory = $true)][string]$Version, [string]$InstalledVersion)
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSIX version must be a numeric major.minor.patch version' }
    $candidate = [version]"$Version.0"
    if ($InstalledVersion) {
        $saved = [version]$InstalledVersion
        if ($candidate.Major -eq $saved.Major -and $candidate.Minor -eq $saved.Minor -and $candidate.Build -eq $saved.Build) {
            if ($saved.Revision -ge 65535) { throw 'MSIX package revision exhausted; increment the application version' }
            $candidate = [version]"$Version.$($saved.Revision + 1)"
        } elseif ($candidate -lt $saved) { throw 'Refusing to package an application downgrade' }
    }
    foreach ($part in @($candidate.Major, $candidate.Minor, $candidate.Build, $candidate.Revision)) {
        if ($part -lt 0 -or $part -gt 65535) { throw 'MSIX version components must be between 0 and 65535' }
    }
    return $candidate.ToString()
}

function Get-WindowsSdkTool {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory = $true)][ValidateSet('makeappx.exe', 'signtool.exe')][string]$Name)
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $kits = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction Stop).KitsRoot10
    $roots = @($kits, (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10')) | Select-Object -Unique
    foreach ($sdkRoot in $roots) {
        $versions = @(Get-ChildItem -LiteralPath (Join-Path $sdkRoot 'bin') -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^10\.0\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending)
        foreach ($version in $versions) {
            $tool = Join-Path $version.FullName "x64\$Name"
            if (Test-Path -LiteralPath $tool -PathType Leaf) { return $tool }
        }
    }
    throw "Windows SDK tool $Name is not installed"
}

function Get-MsixSigningCertificate {
    [CmdletBinding()]
    [OutputType([System.Security.Cryptography.X509Certificates.X509Certificate2])]
    param([string]$Thumbprint)
    if ($Thumbprint -and $Thumbprint -notmatch '^[0-9a-fA-F]{40}$') { throw 'Signing certificate thumbprint must be 40 hexadecimal characters' }
    $certificates = @(Get-ChildItem Cert:\CurrentUser\My | Where-Object {
        $_.HasPrivateKey -and $_.NotBefore -le (Get-Date) -and $_.NotAfter -gt (Get-Date) -and
        $(if ($Thumbprint) { $_.Thumbprint -eq $Thumbprint } else { $_.Subject -eq 'CN=Darbot Labs' })
    })
    $certificates = @($certificates | Where-Object {
        @($_.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' } |
            ForEach-Object { $_.EnhancedKeyUsages } | Where-Object { $_.Value -eq '1.3.6.1.5.5.7.3.3' }).Count -gt 0
    })
    if ($certificates.Count -ne 1) { throw "Expected one valid Darbot Labs code-signing certificate with a private key in CurrentUser\My; found $($certificates.Count). Use -SigningCertificateThumbprint to select one." }
    return $certificates[0]
}

function New-MsixLogo {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Low')]
    param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][int]$Size)
    if (-not $PSCmdlet.ShouldProcess($Destination, 'Create MSIX logo from the existing application icon')) { return }
    Add-Type -AssemblyName System.Drawing
    $image = [System.Drawing.Image]::FromFile($Source)
    $bitmap = New-Object System.Drawing.Bitmap($Size, $Size)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.DrawImage($image, 0, 0, $Size, $Size)
        $bitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose(); $image.Dispose() }
}

function New-GenAudioMsix {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Low')]
    [OutputType([pscustomobject])]
    param([Parameter(Mandatory = $true)][string]$Executable, [Parameter(Mandatory = $true)][string]$Sidecar,
        [Parameter(Mandatory = $true)][string]$Version, [Parameter(Mandatory = $true)][ValidateSet('x64', 'x86', 'arm64')][string]$Architecture,
        [Parameter(Mandatory = $true)][string]$Icon, [Parameter(Mandatory = $true)][string]$OutputDirectory,
        [Parameter(Mandatory = $true)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [Parameter(Mandatory = $true)][string]$RuntimePayload)
    if (-not $PSCmdlet.ShouldProcess($OutputDirectory, 'Create and sign Gen-Audio MSIX')) { return }
    if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSIX version must be a numeric major.minor.patch version' }
    if ($PSVersionTable.PSVersion.Major -ge 7) { Import-Module Appx -UseWindowsPowerShell }
    $installed = @(Get-AppxPackage -Name 'labs.darbot.genaudio' | Where-Object { $_.Publisher -eq $Certificate.Subject } | Sort-Object Version -Descending)
    $saved = if ($installed.Count -gt 0) { $installed[0].Version.ToString() } else { $null }
    $packageVersion = [version](Get-MsixPackageVersion -Version $Version -InstalledVersion $saved)
    foreach ($part in @($packageVersion.Major, $packageVersion.Minor, $packageVersion.Build, $packageVersion.Revision)) {
        if ($part -lt 0 -or $part -gt 65535) { throw 'MSIX version components must be between 0 and 65535' }
    }
    $makeAppx = Get-WindowsSdkTool -Name makeappx.exe
    $signTool = Get-WindowsSdkTool -Name signtool.exe
    $output = [IO.Path]::GetFullPath($OutputDirectory)
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    $stage = Join-Path $output ('stage-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    $path = Join-Path $output "Gen-Audio_${Version}_${Architecture}.msix"
    try {
        Copy-Item -LiteralPath $Executable -Destination (Join-Path $stage 'gen-audio.exe')
        Copy-Item -LiteralPath $Sidecar -Destination (Join-Path $stage 'gen-audio-mcp.exe')
        if (-not (Test-Path -LiteralPath (Join-Path $RuntimePayload 'src\gen_audio\runtime_owner.py') -PathType Leaf)) { throw 'Installed runtime payload is missing' }
        Copy-Item -LiteralPath $RuntimePayload -Destination (Join-Path $stage 'runtime') -Recurse
        # App-local CRT files, when imported, keep installation independent of a
        # separately installed VC redistributable. Windows system DLLs stay OS-owned.
        $imports = @(& dumpbin.exe /dependents $Executable $Sidecar)
        if ($LASTEXITCODE -ne 0) { throw 'dumpbin could not inspect executable dependencies' }
        $runtimeNames = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll', 'msvcp140_2.dll', 'concrt140.dll', 'WebView2Loader.dll')
        foreach ($name in $runtimeNames) {
            if (-not ($imports -match ('(?i)^\s*' + [regex]::Escape($name) + '\s*$'))) { continue }
            $candidates = @((Join-Path (Split-Path -Parent $Executable) $name))
            if ($env:VCToolsRedistDir) {
                $candidates += @(Get-ChildItem -LiteralPath (Join-Path $env:VCToolsRedistDir $Architecture) -Filter $name -Recurse -File | ForEach-Object { $_.FullName })
            }
            $dependency = @($candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1)
            if ($dependency.Count -ne 1) { throw "Required app-local dependency $name was not found" }
            Copy-Item -LiteralPath $dependency[0] -Destination (Join-Path $stage $name)
        }
        $assets = Join-Path $stage 'Assets'
        New-Item -ItemType Directory -Path $assets | Out-Null
        foreach ($logo in @(@('Square44x44Logo.png', 44), @('Square150x150Logo.png', 150), @('StoreLogo.png', 50))) {
            New-MsixLogo -Source $Icon -Destination (Join-Path $assets $logo[0]) -Size $logo[1]
        }
        $publisher = [Security.SecurityElement]::Escape($Certificate.Subject)
        $manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
 xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
 xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
 xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
 IgnorableNamespaces="uap uap10 rescap">
 <Identity Name="labs.darbot.genaudio" Publisher="$publisher" Version="$packageVersion" ProcessorArchitecture="$Architecture" />
 <Properties><DisplayName>Gen-Audio</DisplayName><PublisherDisplayName>Darbot Labs</PublisherDisplayName><Description>Gen-Audio desktop studio</Description><Logo>Assets\StoreLogo.png</Logo></Properties>
 <Resources><Resource Language="en-us" /></Resources>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0" /></Dependencies>
 <Applications><Application Id="GenAudio" Executable="gen-audio.exe" uap10:RuntimeBehavior="packagedClassicApp" uap10:TrustLevel="mediumIL">
  <uap:VisualElements DisplayName="Gen-Audio" Description="Gen-Audio desktop studio" Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" BackgroundColor="transparent" />
 </Application></Applications>
 <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
"@
        [IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $manifest, (New-Object Text.UTF8Encoding($false)))
        Invoke-Checked $makeAppx pack /d $stage /p $path /o
        Invoke-Checked $signTool sign /fd SHA256 /sha1 $Certificate.Thumbprint $path
        Invoke-Checked $signTool verify /pa /v $path
        $certificatePath = Join-Path $output 'DarbotLabs.cer'
        [IO.File]::WriteAllBytes($certificatePath, $Certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert))
        [pscustomobject]@{Path=$path; Version=$packageVersion.ToString(); Identity='labs.darbot.genaudio'; Publisher=$Certificate.Subject;
            Certificate=$certificatePath; CertificateThumbprint=$Certificate.Thumbprint; Sha256=(Get-Sha256 -LiteralPath $path);
            ExeSha256=(Get-Sha256 -LiteralPath $Executable); SidecarSha256=(Get-Sha256 -LiteralPath $Sidecar)}
    } finally {
        $resolved = (Resolve-Path -LiteralPath $stage).Path
        if ($resolved -ne $stage -or -not $resolved.StartsWith($output + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup of an unexpected MSIX staging path' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

function Install-GenAudioMsix {
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param([Parameter(Mandatory = $true)][pscustomobject]$Package)
    if ($PSVersionTable.PSVersion.Major -ge 7) { Import-Module Appx -UseWindowsPowerShell }
    Add-AppxPackage -Path $Package.Path -ForceApplicationShutdown -ErrorAction Stop
    $installed = @(Get-AppxPackage -Name $Package.Identity | Where-Object { $_.Publisher -eq $Package.Publisher })
    if ($installed.Count -ne 1 -or $installed[0].Version.ToString() -ne $Package.Version) { throw 'Installed MSIX identity or version does not match this build' }
    foreach ($binary in @(@('gen-audio.exe', $Package.ExeSha256), @('gen-audio-mcp.exe', $Package.SidecarSha256))) {
        if ((Get-Sha256 -LiteralPath (Join-Path $installed[0].InstallLocation $binary[0])) -ne $binary[1]) { throw "Installed $($binary[0]) does not match the clean build" }
    }
    $runtime = Join-Path $installed[0].InstallLocation 'runtime'
    $inventory = Get-Content -LiteralPath (Join-Path $runtime 'payload-sha256.json') -Raw | ConvertFrom-Json
    foreach ($entry in $inventory.PSObject.Properties) {
        if ((Get-Sha256 -LiteralPath (Join-Path $runtime $entry.Name)) -ne $entry.Value) { throw "Installed runtime payload differs: $($entry.Name)" }
    }
    return $installed[0]
}
