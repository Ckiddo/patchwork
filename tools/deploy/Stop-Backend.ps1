. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
Write-Utf8 "$DeployRoot\control\backend.stop" ([DateTime]::UtcNow.ToString('o'))
$deadline=[DateTime]::UtcNow.AddSeconds(40)
do {
    $listeners=@(Get-NetTCPConnection -State Listen -LocalPort 18120 -ErrorAction SilentlyContinue)
    $task=Get-ScheduledTask -TaskName PatchworkBackend -ErrorAction SilentlyContinue
    if($listeners.Count -eq 0 -and (!$task -or $task.State -ne 'Running')){Write-Output 'Patchwork backend stopped gracefully';exit 0}
    Start-Sleep -Milliseconds 300
} while([DateTime]::UtcNow -lt $deadline)
throw 'Graceful stop timed out; no unrelated process was killed'
