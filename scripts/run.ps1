$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\avrig.exe'
$config = Join-Path $root 'config'
$runtimeDir = Join-Path $root '.run'
$pidFile = Join-Path $runtimeDir 'avrig.pid'
$stdout = Join-Path $runtimeDir 'avrig.stdout.log'
$stderr = Join-Path $runtimeDir 'avrig.stderr.log'

if (-not (Test-Path $exe)) {
    throw "Release executable not found: $exe"
}

New-Item -ItemType Directory -Path $runtimeDir -Force | Out-Null

if (Test-Path $pidFile) {
    $savedPid = 0
    $parsed = [int]::TryParse((Get-Content $pidFile -Raw).Trim(), [ref]$savedPid)
    if ($parsed) {
        $existing = Get-CimInstance Win32_Process -Filter "ProcessId = $savedPid" -ErrorAction SilentlyContinue
        if ($existing -and $existing.ExecutablePath -eq $exe) {
            Write-Output "avrig API is already running (PID $savedPid)."
            Write-Output "Logs: $stdout and $stderr"
            exit 0
        }
    }
    Remove-Item $pidFile -Force
}

$process = Start-Process `
    -FilePath $exe `
    -ArgumentList @('--config', $config, 'serve') `
    -WorkingDirectory $root `
    -WindowStyle Hidden `
    -PassThru `
    -RedirectStandardOutput $stdout `
    -RedirectStandardError $stderr

Start-Sleep -Milliseconds 500
$process.Refresh()
if ($process.HasExited) {
    if (Test-Path $stderr) {
        Get-Content $stderr -Tail 40
    }
    throw "avrig exited during startup (exit code $($process.ExitCode))."
}

Set-Content -Path $pidFile -Value $process.Id -Encoding ascii
Write-Output "Started avrig API in the background (PID $($process.Id))."
Write-Output "Health endpoint: http://127.0.0.1:8000/api/v1/health"
Write-Output "Application log: $(Join-Path $root 'logs\avrig.log')"
