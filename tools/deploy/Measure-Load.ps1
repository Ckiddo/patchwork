param([ValidateRange(10,600)][int]$Seconds=180)
. "$PSScriptRoot\Common.ps1"
$ctx=Get-Content "$DeployRoot\control\acceptance-context.json" -Raw | ConvertFrom-Json
if($ctx.Database -notmatch '^patchwork_test_deploy_[0-9]{14}$' -or $ctx.Port -ne 15432){throw 'Isolated acceptance database required'}
Use-Pg admin $ctx.Database
$dir=Assert-ProjectPath $ctx.Directory
$stop="$dir\load-monitor.stop"
Remove-Item -LiteralPath $stop -ErrorAction SilentlyContinue
$output="$dir\load-samples.jsonl"
Write-Utf8 $output ''
$deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
while([DateTime]::UtcNow -lt $deadline -and !(Test-Path -LiteralPath $stop)){
    $sample=@{At=[DateTime]::UtcNow.ToString('o');LogicalProcessors=[Environment]::ProcessorCount}
    $process=@(Get-Process -Name patchwork-server -ErrorAction SilentlyContinue | Where-Object {$_.Path -like "$DeployRoot\releases\*"})
    $sample.Backend=@($process|ForEach-Object {@{Pid=$_.Id;CpuSeconds=$_.CPU;WorkingSetBytes=$_.WorkingSet64;PrivateBytes=$_.PrivateMemorySize64;Threads=$_.Threads.Count}})
    # Never collect query text, session identities, connection strings or parameters.
    $sample.Database=(Invoke-Sql @'
SELECT json_build_object(
 'connections',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='patchwork_app'),
 'active',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='patchwork_app' AND state='active'),
 'lock_waiters',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='patchwork_app' AND wait_event_type='Lock'),
 'lwlock_waiters',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='patchwork_app' AND wait_event_type='LWLock'),
 'io_waiters',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='patchwork_app' AND wait_event_type='IO'),
 'deadlocks',deadlocks,'commits',xact_commit,'rollbacks',xact_rollback)
FROM pg_stat_database WHERE datname=current_database();
'@)|ConvertFrom-Json
    [IO.File]::AppendAllText($output,($sample|ConvertTo-Json -Depth 5 -Compress)+"`n",$Utf8)
    Start-Sleep -Seconds 1
}
Get-Content -LiteralPath $output
