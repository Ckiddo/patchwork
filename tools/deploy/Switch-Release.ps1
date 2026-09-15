param([Parameter(Mandatory)][ValidatePattern('^[a-zA-Z0-9_.-]+$')][string]$Release)
. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$manifest=Test-Release (Assert-ProjectPath "$DeployRoot\releases\$Release")
if($manifest.Release -ne $Release){throw 'Release directory and manifest mismatch'}
Assert-Schema $manifest
$previous=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
& "$PSScriptRoot\Stop-Backend.ps1"
try {
    Set-ReleasePointer $Release
    Start-ScheduledTask -TaskName PatchworkBackend
    Wait-Backend
} catch {
    & "$PSScriptRoot\Stop-Backend.ps1"
    Set-ReleasePointer $previous
    Start-ScheduledTask -TaskName PatchworkBackend
    Wait-Backend
    throw
}
Write-Output "Active release: $Release"
