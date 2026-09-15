$ErrorActionPreference = 'Stop'
$TestRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$TestArtifacts = Join-Path $TestRoot 'artifacts'
$TestSessionFile = Join-Path $TestArtifacts 'local-test-session.json'
$TestStopFile = Join-Path $TestArtifacts 'browser-preview.stop'
$TestPlayerA = 'http://127.0.0.1:8082/'
$TestPlayerB = 'http://localhost:8082/'

function Get-LocalTestProcess {
    if (-not (Test-Path -LiteralPath $TestSessionFile)) { return $null }
    $record = Get-Content -LiteralPath $TestSessionFile -Raw | ConvertFrom-Json
    $process = Get-Process -Id $record.RunnerPid -ErrorAction SilentlyContinue
    if ($null -eq $process) { return $null }
    if ($process.StartTime.ToUniversalTime().Ticks.ToString() -ne $record.StartedUtcTicks -or
        $process.Path -ne $record.RunnerPath) {
        throw 'The recorded PID now belongs to another process. No process was stopped.'
    }
    return $process
}

function Test-LocalPortFree([int]$Port) {
    $listener = New-Object System.Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $Port)
    try { $listener.Start(); return $true } catch { return $false } finally { $listener.Stop() }
}

function Assert-LocalPortsFree {
    foreach ($port in @(8000, 8082)) {
        if (-not (Test-LocalPortFree $port)) { throw "Port $port is occupied. No unrelated process will be stopped." }
    }
}

function Wait-LocalTestStopped($Process) {
    if (-not $Process.WaitForExit(45000)) {
        throw 'Runner did not stop within 45 seconds. Inspect artifacts/local-test-runner.err.log; process identity has been preserved.'
    }
    Assert-LocalPortsFree
    $log = Join-Path $TestArtifacts 'local-test-runner.log'
    if (-not (Test-Path -LiteralPath $log) -or -not (Select-String -LiteralPath $log -SimpleMatch 'Preview stopped; disposable cluster removed.' -Quiet)) {
        throw 'Runner exited, but database cleanup is not confirmed. Inspect its logs before restarting.'
    }
}
