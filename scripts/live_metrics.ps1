# Live dashboard for capture_out/metrics.csv — run on a second monitor while gaming.
# Usage:  .\scripts\live_metrics.ps1 capture_out\metrics.csv

param(
    [string]$CsvPath = "capture_out\metrics.csv"
)

if (-not (Test-Path $CsvPath)) {
    Write-Host "Waiting for $CsvPath ..."
    while (-not (Test-Path $CsvPath)) { Start-Sleep -Milliseconds 500 }
}

Write-Host "Watching $CsvPath (Ctrl+C to stop)`n"

while ($true) {
    $lines = Get-Content $CsvPath -ErrorAction SilentlyContinue
    if ($lines -and $lines.Count -ge 2) {
        $header = $lines[0]
        $row = $lines[-1]
        Clear-Host
        Write-Host "=== rs-capture-pipeline (this process only) ===" -ForegroundColor Cyan
        Write-Host $header
        Write-Host $row -ForegroundColor Green
        Write-Host "`nTip: compare with OBS using MSI Afterburner (see docs/GAME_BENCHMARK.md)"
    }
    Start-Sleep -Seconds 1
}
