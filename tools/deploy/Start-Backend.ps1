. "$PSScriptRoot\Common.ps1"
try {
    $c=Get-Deployment
    $release=(Get-Content -LiteralPath "$DeployRoot\config\current-release.txt" -Raw).Trim()
    if($release -notmatch '^[a-zA-Z0-9_.-]+$'){throw 'Invalid release pointer'}
    $directory=Assert-ProjectPath "$DeployRoot\releases\$release"
    $manifest=Test-Release $directory
    if($manifest.Release -ne $release){throw 'Release directory and manifest mismatch'}
    Use-Pg 'app'
    $deadline=[DateTime]::UtcNow.AddSeconds(60)
    do {
        if(Test-Path -LiteralPath "$DeployRoot\control\backend.stop"){exit 0}
        try { if((Invoke-Sql 'SELECT 1') -eq '1'){break} } catch {}
        if([DateTime]::UtcNow -ge $deadline){throw 'PostgreSQL startup deadline exceeded'}
        Start-Sleep -Seconds 1
    } while($true)
    # The backend checks exact migration versions and checksums before listening.
    $env:TEMP="$DeployRoot\tmp\backend"; $env:TMP=$env:TEMP
    $stamp=[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    $process=Start-Process -FilePath "$directory\patchwork-server.exe" -ArgumentList @('--config',"$DeployRoot\config\backend.toml") -WorkingDirectory $directory -WindowStyle Hidden -PassThru -RedirectStandardOutput "$DeployRoot\logs\backend\$stamp.out.log" -RedirectStandardError "$DeployRoot\logs\backend\$stamp.err.log"
    # Cache the Windows handle before exit; otherwise PS 5 may report null ExitCode.
    $null=$process.Handle
    Write-Utf8 "$DeployRoot\control\backend-process.json" ((@{Pid=$process.Id;Started=$process.StartTime.ToUniversalTime().ToString('o');Release=$release})|ConvertTo-Json)
    $process.WaitForExit()
    $code=$process.ExitCode
    if($null -eq $code){$code=1}
    Write-Utf8 "$DeployRoot\control\backend-exit.json" ((@{At=[DateTime]::UtcNow.ToString('o');ExitCode=$code;Release=$release})|ConvertTo-Json)
    if($code -ne 0){exit 1}
    exit 0
} catch {
    # Avoid forwarding exceptions that could include configuration values.
    Write-Utf8 "$DeployRoot\control\backend-exit.json" ((@{At=[DateTime]::UtcNow.ToString('o');ExitCode=1;Error='launcher failed'})|ConvertTo-Json)
    exit 1
}
