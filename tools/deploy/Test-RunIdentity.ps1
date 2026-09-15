. "$PSScriptRoot\Common.ps1"
$c=Get-Deployment
if([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -ne $c.BackendSid){throw 'Run this probe as the backend task account'}
function Can-Read([string]$Path){try {$f=[IO.File]::OpenRead($Path);$f.Dispose();return $true}catch [UnauthorizedAccessException]{return $false}}
function Can-Write([string]$Directory){
    $path=Join-Path $Directory "acl-probe-$([Guid]::NewGuid().ToString('N')).tmp"
    try {[IO.File]::WriteAllText($path,'probe');Remove-Item -LiteralPath $path;return $true}catch [UnauthorizedAccessException]{return $false}
}
$r=@{
    CorrectIdentity=$true
    AppKeyReadable=(Can-Read "$DeployRoot\secrets\postgres-app.key")
    AdminKeyDenied=(!(Can-Read "$DeployRoot\secrets\postgres-admin.key"))
    TaskPasswordDenied=(!(Can-Read "$DeployRoot\secrets\backend-task.key"))
    PgDataDenied=(!(Can-Read "$DeployRoot\data\postgresql\PG_VERSION"))
    ToolsWriteDenied=(!(Can-Write "$DeployRoot\tools"))
    ConfigWriteDenied=(!(Can-Write "$DeployRoot\config"))
    LogsWritable=(Can-Write "$DeployRoot\logs\backend")
}
Write-Utf8 "$DeployRoot\control\identity-check.json" ($r|ConvertTo-Json)
if($r.Values -contains $false){exit 1}
