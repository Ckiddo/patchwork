. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$stop="$DeployRoot\control\backend.stop"
Remove-Item -LiteralPath $stop -ErrorAction SilentlyContinue
# Own the retry budget explicitly. Task Scheduler can treat some native child
# exit codes as a successful action; it is used only for logon/startup here.
for($attempt=0;$attempt -le 3;$attempt++){
    Write-Utf8 "$DeployRoot\control\supervisor-state.json" ((@{Attempt=$attempt;MaxRetries=3;At=[DateTime]::UtcNow.ToString('o');State='running'})|ConvertTo-Json)
    & "$PSScriptRoot\Start-Backend.ps1"
    $code=$LASTEXITCODE
    if($code -eq 0 -or (Test-Path -LiteralPath $stop)){exit 0}
    if($attempt -eq 3){
        Write-Utf8 "$DeployRoot\control\supervisor-state.json" ((@{Attempt=$attempt;MaxRetries=3;At=[DateTime]::UtcNow.ToString('o');State='retry_budget_exhausted'})|ConvertTo-Json)
        exit 1
    }
    Write-Utf8 "$DeployRoot\control\supervisor-state.json" ((@{Attempt=$attempt;MaxRetries=3;At=[DateTime]::UtcNow.ToString('o');State='backoff';DelaySeconds=60})|ConvertTo-Json)
    for($second=0;$second -lt 60;$second++){
        if(Test-Path -LiteralPath $stop){exit 0}
        Start-Sleep -Seconds 1
    }
    if(Test-Path -LiteralPath $stop){exit 0}
}
