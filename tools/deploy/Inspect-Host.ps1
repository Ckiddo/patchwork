$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'"
$services = @(Get-CimInstance Win32_Service | Where-Object { $_.Name -match 'postgres|patchwork' -or $_.DisplayName -match 'postgres|patchwork' } | Select-Object Name,State,StartName,StartMode,PathName)
$ports = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -in 15432,18120,15433,18121 } | Select-Object LocalAddress,LocalPort,OwningProcess)
$candidates = @('D:\Tools\PostgreSQL','C:\Program Files\PostgreSQL','D:\deploy_patchwork')
$directories = @($candidates | ForEach-Object { if (Test-Path -LiteralPath $_) { Get-ChildItem -LiteralPath $_ -Directory | Select-Object FullName } })
[ordered]@{
    Hostname=$env:COMPUTERNAME; User=$identity.Name; Admin=$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    OS=(Get-CimInstance Win32_OperatingSystem | Select-Object Caption,OSArchitecture,FreePhysicalMemory)
    Disk=($disk | Select-Object DeviceID,FileSystem,Size,FreeSpace)
    Services=$services; Ports=$ports; Directories=$directories
    TargetExists=(Test-Path -LiteralPath 'D:\deploy_patchwork')
    Tasks=@(Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object TaskName -Like 'Patchwork*' | Select-Object TaskName,State)
} | ConvertTo-Json -Depth 5
