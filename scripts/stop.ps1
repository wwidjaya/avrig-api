$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\avrig.exe'
$pidFile = Join-Path $root '.run\avrig.pid'

if (-not (Test-Path $pidFile)) {
    Write-Output 'No make-run PID file found; avrig is not managed by make run.'
    exit 0
}

$savedPid = 0
if (-not [int]::TryParse((Get-Content $pidFile -Raw).Trim(), [ref]$savedPid)) {
    throw "Invalid PID in $pidFile"
}

$process = Get-CimInstance Win32_Process -Filter "ProcessId = $savedPid" -ErrorAction SilentlyContinue
if (-not $process) {
    Remove-Item $pidFile -Force
    Write-Output 'The recorded avrig process is already stopped.'
    exit 0
}

if ($process.ExecutablePath -ne $exe) {
    throw "PID $savedPid belongs to '$($process.ExecutablePath)', not this project's avrig executable. Refusing to stop it."
}

Stop-Process -Id $savedPid -Force
Wait-Process -Id $savedPid -Timeout 10 -ErrorAction SilentlyContinue
Remove-Item $pidFile -Force
Write-Output "Stopped avrig API (PID $savedPid)."
