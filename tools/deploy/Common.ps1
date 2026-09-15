$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$DeployRoot = 'D:\deploy_patchwork'
$Utf8 = New-Object Text.UTF8Encoding($false)
function Write-Utf8([string]$Path, [string]$Text) { [IO.File]::WriteAllText($Path, $Text, $Utf8) }
function Assert-ProjectPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if ($full -ne $DeployRoot -and -not $full.StartsWith($DeployRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Path outside Patchwork deployment' }
    return $full
}
function Get-Deployment {
    $config = Get-Content -LiteralPath "$DeployRoot\config\deployment.json" -Raw | ConvertFrom-Json
    if ($config.Project -ne 'patchwork' -or $config.Hostname -ne $env:COMPUTERNAME -or $config.Root -ne $DeployRoot) { throw 'Deployment ownership mismatch' }
    return $config
}
function Invoke-Native([string]$Program, [string[]]$Arguments, [string]$Log) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $output = & $Program @Arguments 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $saved }
    if ($Log) { $output | Out-File -LiteralPath $Log -Encoding utf8 -Append }
    if ($code -ne 0) { throw "Native operation failed ($([IO.Path]::GetFileName($Program)), exit $code); inspect protected deployment log" }
    return $output
}
function Invoke-PgControl([string[]]$Arguments,[string]$Log) {
    # A persistent postgres child can inherit PowerShell pipeline handles.
    # Redirect to files and wait for pg_ctl itself, not its descendant processes.
    $quoted=@($Arguments|ForEach-Object {'"'+$_.Replace('"','\"')+'"'}) -join ' '
    $p=Start-Process -FilePath "$PgBin\pg_ctl.exe" -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Log.ctl-out.log" -RedirectStandardError "$Log.ctl-err.log"
    $null=$p.Handle
    if(!$p.WaitForExit(150000)){throw 'pg_ctl exceeded its startup/shutdown deadline; inspect isolated process state'}
    if($p.ExitCode -ne 0){throw 'pg_ctl failed; inspect protected control logs'}
}
function Use-Pg([string]$Role='admin', [string]$Database='patchwork', [int]$Port=15432) {
    $c = Get-Deployment
    $script:PgBin = $c.PgBin
    $env:PGHOST='127.0.0.1'; $env:PGPORT=[string]$Port; $env:PGDATABASE=$Database
    $env:PGUSER="patchwork_$Role"; $env:PGPASSFILE="$DeployRoot\secrets\postgres-$Role.pgpass"
    $env:PGCONNECT_TIMEOUT='5'; $env:PGCLIENTENCODING='UTF8'
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
}
function Invoke-Sql([string]$Sql) {
    $saved = $ErrorActionPreference; $ErrorActionPreference='Continue'
    try { $output = $Sql | & "$PgBin\psql.exe" -X -qAt -v ON_ERROR_STOP=1 2>&1; $code=$LASTEXITCODE } finally { $ErrorActionPreference=$saved }
    if ($code -ne 0) { throw 'Database operation failed; private SQL output suppressed' }
    return ($output -join "`n").Trim()
}
function Set-PrivateAcl([string]$Path, [hashtable]$Extra=@{}) {
    $item=Get-Item -LiteralPath $Path
    $acl=if($item.PSIsContainer){New-Object Security.AccessControl.DirectorySecurity}else{New-Object Security.AccessControl.FileSecurity}
    $acl.SetAccessRuleProtection($true,$false)
    $rules=@{'S-1-5-18'='FullControl';'S-1-5-32-544'='FullControl'}
    foreach($key in $Extra.Keys){$rules[$key]=$Extra[$key]}
    foreach($key in $rules.Keys){
        $sid=New-Object Security.Principal.SecurityIdentifier($key)
        $inherit=if($item.PSIsContainer){[Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'}else{[Security.AccessControl.InheritanceFlags]::None}
        $rule=New-Object Security.AccessControl.FileSystemAccessRule($sid,[Security.AccessControl.FileSystemRights]$rules[$key],$inherit,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow)
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}
function Wait-Backend([int]$Seconds=60) {
    $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        try { $r=Invoke-RestMethod -Uri 'http://127.0.0.1:18120/readyz' -TimeoutSec 2; if($r.database -eq 'ok'){return} } catch {}
        Start-Sleep -Milliseconds 300
    } while([DateTime]::UtcNow -lt $deadline)
    throw 'Patchwork backend readiness deadline exceeded'
}
function Test-Release([string]$Directory) {
    $directory=Assert-ProjectPath $Directory
    $manifest=Get-Content -LiteralPath "$directory\manifest.json" -Raw | ConvertFrom-Json
    if($manifest.Project -ne 'patchwork' -or $manifest.Release -notmatch '^[a-zA-Z0-9_.-]+$' -or $manifest.PostgresMajor -ne 18){throw 'Invalid release manifest'}
    foreach($required in @('patchwork-server.exe','patchwork-migrate.exe')){
        if($required -notin $manifest.Files.Path){throw 'Required binary missing from manifest'}
    }
    foreach($file in $manifest.Files){
        $path=[IO.Path]::GetFullPath((Join-Path $directory $file.Path))
        if(-not $path.StartsWith($directory+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe release manifest path'}
        if((Get-Item -LiteralPath $path).Length -ne $file.Bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $file.Sha256){throw 'Release integrity mismatch'}
    }
    return $manifest
}
function Assert-Schema($Manifest) {
    Use-Pg 'admin'
    $actual=(Invoke-Sql "SELECT coalesce(json_agg(json_build_object('version',version,'checksum',encode(checksum,'hex'),'success',success) ORDER BY version),'[]'::json) FROM patchwork._sqlx_migrations")|ConvertFrom-Json
    if(@($actual).Count -ne @($Manifest.Schema).Count){throw 'Release incompatible with live schema count'}
    foreach($expected in $Manifest.Schema){
        $row=@($actual|Where-Object {$_.version -eq $expected.version})
        if($row.Count -ne 1 -or -not $row[0].success -or $row[0].checksum -ne $expected.checksum){throw 'Release incompatible with live schema checksum'}
    }
}
function Set-ReleasePointer([string]$Release) {
    if($Release -notmatch '^[a-zA-Z0-9_.-]+$'){throw 'Invalid release pointer'}
    $target="$DeployRoot\config\current-release.txt"
    $pending="$DeployRoot\config\current-release.$([Guid]::NewGuid().ToString('N')).tmp"
    Write-Utf8 $pending $Release
    if(Test-Path -LiteralPath $target){[IO.File]::Replace($pending,$target,"$DeployRoot\config\previous-release.txt")}else{[IO.File]::Move($pending,$target)}
}
