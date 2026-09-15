param(
    [Parameter(Mandatory)][string]$Zip,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$Sha256
)
. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$zipPath=Assert-ProjectPath $Zip
if((Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash -ne $Sha256){throw 'Release archive checksum mismatch'}
$stage=Assert-ProjectPath "$DeployRoot\staging\release-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $stage | Out-Null
# Reject path traversal before extraction, independently of ZIP implementation.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive=[IO.Compression.ZipFile]::OpenRead($zipPath)
try {
    foreach($entry in $archive.Entries){
        $destination=[IO.Path]::GetFullPath((Join-Path $stage $entry.FullName))
        if(-not $destination.StartsWith($stage+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe release ZIP entry'}
    }
} finally {$archive.Dispose()}
Expand-Archive -LiteralPath $zipPath -DestinationPath $stage
$manifest=Test-Release $stage
$release=$manifest.Release
$destination=Assert-ProjectPath "$DeployRoot\releases\$release"
if(Test-Path -LiteralPath $destination){throw 'Release ID already exists'}
Move-Item -LiteralPath $stage -Destination $destination
$pointer="$DeployRoot\config\current-release.txt"
$previous=if(Test-Path -LiteralPath $pointer){(Get-Content -LiteralPath $pointer -Raw).Trim()}else{$null}
if($previous){& "$PSScriptRoot\New-Backup.ps1" -Kind Logical; if($LASTEXITCODE -ne 0){throw 'Pre-release backup failed'}}
& "$PSScriptRoot\Stop-Backend.ps1"
try {
    $null=Invoke-Native "$destination\patchwork-migrate.exe" @('--config',"$DeployRoot\config\migration.toml") "$DeployRoot\logs\migrate-$release.log"
    Assert-Schema $manifest
    Set-ReleasePointer $release
    Start-ScheduledTask -TaskName PatchworkBackend
    Wait-Backend
    Write-Utf8 "$DeployRoot\config\last-publish.json" ((@{Release=$release;Previous=$previous;At=[DateTime]::UtcNow.ToString('o');Ready=$true})|ConvertTo-Json)
    Write-Output "Release ready: $release"
} catch {
    if($previous){
        # Restore binary pointer only when the former binary understands the live schema.
        & "$PSScriptRoot\Stop-Backend.ps1"
        $old=Test-Release "$DeployRoot\releases\$previous"
        Assert-Schema $old
        Set-ReleasePointer $previous
        Start-ScheduledTask -TaskName PatchworkBackend
        Wait-Backend
    }
    throw
}
