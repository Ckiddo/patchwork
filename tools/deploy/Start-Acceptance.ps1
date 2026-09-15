. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$release=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
$directory=Assert-ProjectPath "$DeployRoot\releases\$release"
$null=Test-Release $directory
$env:TEMP="$DeployRoot\tmp\backend";$env:TMP=$env:TEMP
Remove-Item -LiteralPath "$DeployRoot\control\acceptance.stop" -ErrorAction SilentlyContinue
$stamp=[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
$process=Start-Process "$directory\patchwork-server.exe" -ArgumentList @('--config',"$DeployRoot\config\acceptance.toml") -WorkingDirectory $directory -WindowStyle Hidden -PassThru -RedirectStandardOutput "$DeployRoot\logs\backend\acceptance-$stamp.out.log" -RedirectStandardError "$DeployRoot\logs\backend\acceptance-$stamp.err.log"
$null=$process.Handle
$process.WaitForExit()
if($null -eq $process.ExitCode){exit 1}
exit $process.ExitCode
