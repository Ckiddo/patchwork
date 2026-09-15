$ErrorActionPreference='Stop'
$edge='D:\deploy_patchwork\edge'
$utf8=New-Object Text.UTF8Encoding($false)
$stop="$edge\control\stop"
function Record-State([hashtable]$State){
    $State.At=[DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText("$edge\control\state.json",($State|ConvertTo-Json),$utf8)
}
try {
    $config=Get-Content "$edge\deployment.json" -Raw|ConvertFrom-Json
    if($config.Project -ne 'patchwork' -or $config.Host -ne $env:COMPUTERNAME -or $config.Executable -notmatch '^D:\\Tools\\cloudflared\\[0-9.]+\\cloudflared.exe$'){throw 'Tunnel ownership mismatch'}
    if((Get-FileHash $config.Executable).Hash -ne $config.Sha256 -or (Get-FileHash "$edge\config.yml").Hash -ne $config.ConfigSha256){throw 'Tunnel integrity mismatch'}
    Remove-Item -LiteralPath $stop -ErrorAction SilentlyContinue
    for($attempt=0;$attempt -le 3;$attempt++){
        $stamp=[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
        # Credentials are read from an ACL-protected file; neither argv nor
        # environment contains a tunnel token. Fatal-only logging omits request URLs.
        $process=Start-Process -FilePath $config.Executable -ArgumentList @('tunnel','--config',"$edge\config.yml",'run',$config.TunnelId) -WindowStyle Hidden -PassThru -RedirectStandardOutput "$edge\logs\$stamp.out.log" -RedirectStandardError "$edge\logs\$stamp.err.log"
        $null=$process.Handle
        Record-State @{State='running';Pid=$process.Id;Attempt=$attempt}
        while(!$process.WaitForExit(500)){
            if(Test-Path -LiteralPath $stop){
                # The named connector owns no durable game state. Killing only
                # its recorded child closes sockets; browser sessions reconnect.
                $process.Kill();$process.WaitForExit()
                Record-State @{State='stopped';Attempt=$attempt};exit 0
            }
        }
        if(Test-Path -LiteralPath $stop){Record-State @{State='stopped';Attempt=$attempt};exit 0}
        if($attempt -eq 3){Record-State @{State='retry_budget_exhausted';ExitCode=$process.ExitCode;Attempt=$attempt};exit 1}
        Record-State @{State='backoff';ExitCode=$process.ExitCode;Attempt=$attempt;DelaySeconds=60}
        for($second=0;$second -lt 60;$second++){
            if(Test-Path -LiteralPath $stop){Record-State @{State='stopped';Attempt=$attempt};exit 0}
            Start-Sleep -Seconds 1
        }
    }
} catch {
    Record-State @{State='launcher_failed'}
    exit 1
}
