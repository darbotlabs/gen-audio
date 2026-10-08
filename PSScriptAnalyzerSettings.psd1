# PSScriptAnalyzer settings for every PowerShell file in this repo.
# Used by scripts/test.ps1 and the windows CI job (PSScriptAnalyzer 1.24.0).
# Default rule set, Error and Warning severity; any finding fails the run.
@{
    Severity = @('Error', 'Warning')
    Rules    = @{
        PSUseCompatibleSyntax = @{
            Enable         = $true
            TargetVersions = @('5.1', '7.0')
        }
    }
}
