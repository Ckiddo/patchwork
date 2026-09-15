. (Join-Path $PSScriptRoot 'LocalTest.Common.ps1')
try {
    $runner = Get-LocalTestProcess
    if ($null -eq $runner) {
        Assert-LocalPortsFree
        Write-Host 'Local test is already stopped.'
        exit 0
    }
    Write-Host 'Stopping this checkout local test; waiting for its services and database cleanup.'
    Set-Content -LiteralPath $TestStopFile -Value 'stop'
    Wait-LocalTestStopped $runner
    Write-Host 'Stopped. Ports 8000 and 8082 are free; the disposable database has been removed.'
} catch {
    Write-Error $_
    exit 1
}
