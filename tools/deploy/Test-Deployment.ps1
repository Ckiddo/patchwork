. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
if(Get-ScheduledTask -TaskName PatchworkAcceptance -ErrorAction SilentlyContinue){throw 'Finish isolated acceptance before deployment lifecycle checks'}
Wait-Backend
$unrelated=Get-CimInstance Win32_Service -Filter "Name='re-xianyu-postgresql'"|Select-Object Name,ProcessId,State
$report=@{}
try {
    & "$PSScriptRoot\Stop-Deployment.ps1" -WithDatabase | Out-Null
    if(@(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue|Where-Object {$_.LocalPort -in 15432,18120}).Count){throw 'Project listener leaked after stop'}
    & "$PSScriptRoot\Start-Deployment.ps1" | Out-Null
    $report.StartStop=$true
    # Exercise actual abnormal termination and the launcher's bounded restart.
    $listener=Get-NetTCPConnection -State Listen -LocalPort 18120
    $process=Get-CimInstance Win32_Process -Filter "ProcessId=$($listener.OwningProcess)"
    $release=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
    $expected=Assert-ProjectPath "$DeployRoot\releases\$release\patchwork-server.exe"
    if($process.ExecutablePath -ne $expected){throw 'Refusing to terminate an unowned process'}
    $timer=[Diagnostics.Stopwatch]::StartNew()
    Stop-Process -Id $process.ProcessId -Force
    Start-Sleep -Seconds 2
    $exit=Get-Content "$DeployRoot\control\backend-exit.json" -Raw|ConvertFrom-Json
    if($null -eq $exit.ExitCode -or $exit.ExitCode -eq 0){throw 'Launcher failed to preserve abnormal child exit code'}
    Wait-Backend 100
    $newPid=(Get-NetTCPConnection -State Listen -LocalPort 18120).OwningProcess
    if($newPid -eq $process.ProcessId){throw 'Restart did not replace the terminated process'}
    $report.AutomaticRestart=@{Seconds=$timer.Elapsed.TotalSeconds;OldPid=$process.ProcessId;NewPid=$newPid;ExitCode=$exit.ExitCode}
    $after=Get-CimInstance Win32_Service -Filter "Name='re-xianyu-postgresql'"|Select-Object Name,ProcessId,State
    if($after.ProcessId -ne $unrelated.ProcessId -or $after.State -ne $unrelated.State){throw 'Unrelated service changed during checks'}
    $report.UnrelatedPostgresUnchanged=$true
    $report.Ready=Invoke-RestMethod http://127.0.0.1:18120/readyz
    $report.Passed=$true
} finally {
    & "$PSScriptRoot\Start-Deployment.ps1" | Out-Null
    Write-Utf8 "$DeployRoot\control\deployment-test.json" ($report|ConvertTo-Json -Depth 5)
}
$report|ConvertTo-Json -Depth 5
