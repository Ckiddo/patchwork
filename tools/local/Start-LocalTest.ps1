param(
    [switch]$SkipBuild,
    [switch]$NoBrowser,
    [switch]$Acceptance,
    [string]$Python = 'D:\Tools\Python\python.exe',
    [string]$PostgresBin = 'D:\Tools\PostgreSQL\18.6\pgsql\bin'
)
. (Join-Path $PSScriptRoot 'LocalTest.Common.ps1')

# Serialize starts for this checkout, including compilation and database initialization.
$mutexName = 'Local\PatchworkLocalTest-' + [Convert]::ToBase64String(
    [Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($TestRoot))).Replace('/', '_')
$mutex = New-Object Threading.Mutex($false, $mutexName)
$locked = $false
$runner = $null
$launched = $false
try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (-not $locked) { throw 'A start operation is already running. Wait for its readiness message.' }
    $runner = Get-LocalTestProcess
    if ($null -ne $runner) {
        Write-Host "Local test is already running (PID $($runner.Id))."
    } else {
        Assert-LocalPortsFree
        foreach ($path in @($Python, (Join-Path $PostgresBin 'initdb.exe'), (Join-Path $PostgresBin 'pg_ctl.exe'))) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing runtime: $path" }
        }
        New-Item -ItemType Directory -Path $TestArtifacts -Force | Out-Null
        $preview = Join-Path $TestArtifacts 'browser-dist'
        if (-not $SkipBuild) {
            Write-Host 'Building backend and frontend. Incremental builds usually take 1-5 minutes; logs are in artifacts/local-test-build.log.'
            $savedJobs = $env:CARGO_BUILD_JOBS
            $savedColor = $env:NO_COLOR
            $savedApi = $env:PATCHWORK_API_BASE
            $savedLocalTest = $env:PATCHWORK_LOCAL_TEST
            $savedBevyAssets = $env:BEVY_ASSET_PATH
            $savedPath = $env:PATH
            Push-Location $TestRoot
            try {
                $env:CARGO_BUILD_JOBS = '2'
                $env:NO_COLOR = 'true'
                $env:PATCHWORK_API_BASE = 'http://127.0.0.1:8000/api'
                $env:PATCHWORK_LOCAL_TEST = 'true'
                $env:BEVY_ASSET_PATH = Join-Path $TestRoot 'assets'
                # Existing machine installations; overrides remain process-local.
                $env:PATH = 'E:\tools\Rust\cargo\bin;C:\Users\81564\.cargo\bin;' + $savedPath
                Get-Command cargo,trunk -ErrorAction Stop | Out-Null
                # Windows PowerShell 5 treats normal native stderr as ErrorRecords.
                # Judge these builds by exit code while preserving their full log.
                $ErrorActionPreference = 'Continue'
                & cargo build --locked -p backend *> (Join-Path $TestArtifacts 'local-test-build.log')
                $backendExit = $LASTEXITCODE
                $ErrorActionPreference = 'Stop'
                if ($backendExit -ne 0) { throw 'Backend build failed; see artifacts/local-test-build.log.' }
                $ErrorActionPreference = 'Continue'
                & trunk build --release --locked --public-url / --dist $preview *>> (Join-Path $TestArtifacts 'local-test-build.log')
                $frontendExit = $LASTEXITCODE
                $ErrorActionPreference = 'Stop'
                if ($frontendExit -ne 0) { throw 'Frontend build failed; see artifacts/local-test-build.log.' }
            } finally {
                $ErrorActionPreference = 'Stop'
                Pop-Location
                $env:CARGO_BUILD_JOBS = $savedJobs
                $env:NO_COLOR = $savedColor
                $env:PATCHWORK_API_BASE = $savedApi
                $env:PATCHWORK_LOCAL_TEST = $savedLocalTest
                $env:BEVY_ASSET_PATH = $savedBevyAssets
                $env:PATH = $savedPath
            }
        }
        foreach ($path in @((Join-Path $preview 'index.html'), (Join-Path $TestRoot 'target/debug/patchwork-server.exe'), (Join-Path $TestRoot 'target/debug/patchwork-migrate.exe'))) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Build artifacts are missing. Run without -SkipBuild.' }
        }
        $arguments = @((Join-Path $TestRoot 'tools/ci/postgres_suite.py'), '--bin-dir', $PostgresBin,
            '--preview-dir', $preview, '--preview-port', '8082', '--managed-preview')
        if ($Acceptance) { $arguments += '--acceptance' }
        if (Test-Path -LiteralPath $TestStopFile) { Remove-Item -LiteralPath $TestStopFile }
        # These are native argv paths, never shell fragments. Quotes cannot occur in Windows paths.
        $argumentLine = ($arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
        $runner = Start-Process -FilePath $Python -ArgumentList $argumentLine -WorkingDirectory $TestRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $TestArtifacts 'local-test-runner.log') `
            -RedirectStandardError (Join-Path $TestArtifacts 'local-test-runner.err.log')
        $launched = $true
        $record = @{
            RunnerPid = $runner.Id; StartedUtcTicks = $runner.StartTime.ToUniversalTime().Ticks.ToString()
            RunnerPath = (Resolve-Path -LiteralPath $Python).Path
            PlayerA = $TestPlayerA; PlayerB = $TestPlayerB; Api = 'http://127.0.0.1:8000/api'
        }
        $record | ConvertTo-Json | Set-Content -LiteralPath $TestSessionFile -Encoding UTF8
        Write-Host 'Starting isolated PostgreSQL and local services (usually 5-15 seconds).'
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $ready = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($runner.HasExited) { throw 'Runner exited; see artifacts/local-test-runner.err.log.' }
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:8000/readyz' -TimeoutSec 1
            $page = Invoke-WebRequest -UseBasicParsing -Uri $TestPlayerA -TimeoutSec 1
            if ($response.StatusCode -eq 200 -and $page.StatusCode -eq 200) { $ready = $true; break }
        } catch { }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) { throw 'Services did not become ready within 60 seconds; inspect the runner logs.' }
    Write-Host "Ready: $TestPlayerA and $TestPlayerB"
    if (-not $NoBrowser) {
        [Diagnostics.Process]::Start($TestPlayerA) | Out-Null
        [Diagnostics.Process]::Start($TestPlayerB) | Out-Null
    }
} catch {
    if ($launched -and $null -ne $runner -and -not $runner.HasExited) {
        Set-Content -LiteralPath $TestStopFile -Value 'stop'
        $runner.WaitForExit(45000) | Out-Null
    }
    Write-Error $_
    exit 1
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
