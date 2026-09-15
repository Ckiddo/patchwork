param([Parameter(Mandatory)][string]$Source,[Parameter(Mandatory)][string]$Name)
. "$PSScriptRoot\Common.ps1"
$stage='validate'
function Open-WalRead([string]$Path) {
    # PostgreSQL may retain a Windows write handle after a segment is sealed.
    [IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]'ReadWrite,Delete')
}
function Get-WalHash([string]$Path) {
    $stream=Open-WalRead $Path
    $sha=[Security.Cryptography.SHA256]::Create()
    try {([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-','')} finally {$sha.Dispose();$stream.Dispose()}
}
try {
    if($Name -notmatch '^[0-9A-F]{24}(\.[0-9A-F]{8}\.backup)?$|^[0-9A-F]{8}\.history$'){throw 'Invalid WAL name'}
    if(-not [IO.Path]::IsPathRooted($Source)){$Source=Join-Path "$DeployRoot\data\postgresql" $Source}
    $sourcePath=Assert-ProjectPath ([IO.Path]::GetFullPath($Source))
    $target="$DeployRoot\backups\wal\$Name"
    $stage='source_hash'
    $hash=Get-WalHash $sourcePath
    if(Test-Path -LiteralPath $target){
        if((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $hash){throw 'WAL name collision'}
    } else {
        $stage='copy'
        $temp="$target.$([Guid]::NewGuid().ToString('N')).partial"
        $inputStream=Open-WalRead $sourcePath
        try {
            $outputStream=New-Object IO.FileStream($temp,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None,65536,[IO.FileOptions]::WriteThrough)
            try {$inputStream.CopyTo($outputStream);$outputStream.Flush($true)} finally {$outputStream.Dispose()}
        } finally {$inputStream.Dispose()}
        $stage='verify'
        if((Get-FileHash -LiteralPath $temp -Algorithm SHA256).Hash -ne $hash){throw 'WAL copy verification failed'}
        [IO.File]::Move($temp,$target)
    }
    $stage='status'
    Write-Utf8 "$DeployRoot\logs\archive-health.json" ((@{Ok=$true;At=[DateTime]::UtcNow.ToString('o');Wal=$Name})|ConvertTo-Json -Compress)
    exit 0
} catch {
    # A nonzero exit makes PostgreSQL retain and retry this segment.
    $errorType=$_.Exception.GetType().Name;$errorCode=$_.Exception.HResult
    try { Write-Utf8 "$DeployRoot\logs\archive-health.json" ((@{Ok=$false;At=[DateTime]::UtcNow.ToString('o');Wal=$Name;Stage=$stage;ErrorType=$errorType;ErrorCode=$errorCode})|ConvertTo-Json -Compress) } catch {}
    exit 1
}
