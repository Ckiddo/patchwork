. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
if(-not [Diagnostics.EventLog]::SourceExists('PatchworkBackup')){New-EventLog -LogName Application -Source PatchworkBackup}
$settings=New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -RestartCount 2 -RestartInterval (New-TimeSpan -Minutes 5) -ExecutionTimeLimit (New-TimeSpan -Hours 2) -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
foreach($job in @(@{Name='PatchworkLogicalBackup';Kind='Logical';Trigger=(New-ScheduledTaskTrigger -Daily -At '03:00')},@{Name='PatchworkBaseBackup';Kind='Base';Trigger=(New-ScheduledTaskTrigger -Weekly -DaysOfWeek Sunday -At '03:30')})){
    $action=New-ScheduledTaskAction -Execute powershell.exe -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\New-Backup.ps1 -Kind $($job.Kind)" -WorkingDirectory $DeployRoot
    $null=Register-ScheduledTask -TaskName $job.Name -Action $action -Trigger $job.Trigger -Settings $settings -User SYSTEM -RunLevel Highest -Force
}
$action=New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Test-BackupHealth.ps1' -WorkingDirectory $DeployRoot
$trigger=New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 5)
$null=Register-ScheduledTask -TaskName PatchworkBackupMonitor -Action $action -Trigger $trigger -Settings $settings -User SYSTEM -RunLevel Highest -Force
Write-Output 'Installed daily logical, weekly base, and five-minute local backup monitor tasks'
