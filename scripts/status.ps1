$ErrorActionPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$pidFile = Join-Path $root '.run\avrig.pid'
$exe = Join-Path $root 'target\release\avrig.exe'

Write-Host "=== avrig API Service Status (Windows) ===" -ForegroundColor Cyan

if (Test-Path $pidFile) {
    $savedPid = 0
    if ([int]::TryParse((Get-Content $pidFile -Raw).Trim(), [ref]$savedPid)) {
        $p = Get-CimInstance Win32_Process -Filter "ProcessId = $savedPid" -ErrorAction SilentlyContinue
        if ($p -and $p.ExecutablePath -eq $exe) {
            Write-Host "Process: Running (PID $savedPid)" -ForegroundColor Green
        } else {
            Write-Host "Process: Not running (stale PID file: $savedPid)" -ForegroundColor Yellow
        }
    }
} else {
    Write-Host "Process: Not running (no PID file)"
}

Write-Host "`n=== Health Endpoint Checks ===" -ForegroundColor Cyan
try {
    $local = Invoke-WebRequest -Uri 'http://127.0.0.1:8000/api/v1/health' -TimeoutSec 2 -UseBasicParsing
    Write-Host "Direct local loopback: HTTP $($local.StatusCode)" -ForegroundColor Green
} catch {
    Write-Host "Direct local loopback: Connection failed ($($_.Exception.Message))" -ForegroundColor Red
}

try {
    $proxy = Invoke-WebRequest -Uri 'https://api.avrig.ai/api/v1/health' -TimeoutSec 2 -UseBasicParsing
    Write-Host "Reverse proxy endpoint (https://api.avrig.ai): HTTP $($proxy.StatusCode)" -ForegroundColor Green
} catch {
    Write-Host "Reverse proxy endpoint (https://api.avrig.ai): Connection failed ($($_.Exception.Message))" -ForegroundColor Red
}
