. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$source="$DeployRoot\staging\cloudflared"
$manifest=Get-Content "$source\manifest.json" -Raw | ConvertFrom-Json
if($manifest.Project -ne 'cloudflared' -or $manifest.Version -notmatch '^\d{4}\.\d{1,2}\.\d+$' -or $manifest.Sha256 -notmatch '^[a-f0-9]{64}$'){throw 'Invalid connector manifest'}
if($manifest.Source -ne "https://github.com/cloudflare/cloudflared/releases/download/$($manifest.Version)/cloudflared-windows-amd64.exe"){throw 'Unexpected connector release source'}
$binary="$source\cloudflared.exe"
if((Get-FileHash -LiteralPath $binary).Hash -ne $manifest.Sha256 -or (Get-Item -LiteralPath $binary).Length -ne $manifest.Bytes){throw 'Connector digest mismatch'}
$directory="D:\Tools\cloudflared\$($manifest.Version)"
if(Test-Path -LiteralPath "$directory\cloudflared.exe"){
    if((Get-FileHash "$directory\cloudflared.exe").Hash -ne $manifest.Sha256){throw 'Existing connector differs; inspect before replacing'}
} else {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination "$directory\cloudflared.exe"
    Copy-Item -LiteralPath "$source\manifest.json" -Destination "$directory\manifest.json"
}
$version=& "$directory\cloudflared.exe" --version
if($LASTEXITCODE -ne 0 -or $version -notlike "cloudflared version $($manifest.Version)*"){throw 'Connector version check failed'}
@{Version=$manifest.Version;Sha256=$manifest.Sha256;Executable="$directory\cloudflared.exe";Installed=$true}|ConvertTo-Json
