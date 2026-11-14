$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $root 'target\release\avrig.exe'
$destinationDir = 'D:\avrig'
$destination = Join-Path $destinationDir 'avrig.exe'
$staged = Join-Path $destinationDir ("avrig.exe.deploy-$([guid]::NewGuid().ToString('N')).tmp")

if (-not (Test-Path $source)) {
    throw "Release executable not found: $source"
}

$serviceName = 'avrig'
$legacyServiceName = 'avrig-watcher'
$serviceDisplayName = 'avrig Windows Runtime Node'
$serviceDescription = 'Unified cross-platform avrig runtime node hosting directory ingestion watcher and hardware-accelerated (XPU) neural cross-encoder reranker.'
$nssmCmd = Get-Command nssm -ErrorAction SilentlyContinue

# If legacy or current NSSM service exists, stop it before replacing the binary
if ($nssmCmd) {
    foreach ($name in @($serviceName, $legacyServiceName)) {
        $svc = Get-Service -Name $name -ErrorAction SilentlyContinue
        if ($svc -and $svc.Status -eq 'Running') {
            Write-Host "Stopping Windows service $name..." -ForegroundColor Cyan
            & $nssmCmd.Source stop $name 2>$null
            Start-Sleep -Seconds 1
        }
    }
}

$runningCopies = Get-Process avrig -ErrorAction SilentlyContinue |
    ForEach-Object {
        Get-CimInstance Win32_Process -Filter "ProcessId = $($_.Id)" -ErrorAction SilentlyContinue
    } |
    Where-Object { $_.ExecutablePath -eq $destination }

if ($runningCopies) {
    $pids = ($runningCopies | ForEach-Object { $_.ProcessId }) -join ', '
    throw "Cannot replace $destination while it is running (PID $pids). Stop that avrig process or service, then retry make deploy."
}

New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
try {
    Copy-Item -Path $source -Destination $staged -Force
    Move-Item -Path $staged -Destination $destination -Force
}
finally {
    if (Test-Path $staged) {
        Remove-Item $staged -Force
    }
}

$deployed = Get-Item $destination
$built = Get-Item $source
function Get-Sha256([string]$Path) {
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return ([System.BitConverter]::ToString($algorithm.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $stream.Dispose()
        $algorithm.Dispose()
    }
}

$sourceHash = Get-Sha256 $source
$destinationHash = Get-Sha256 $destination
if ($sourceHash -ne $destinationHash) {
    throw "Deployment SHA-256 check failed: source=$sourceHash, destination=$destinationHash"
}

Write-Output "Deployed Windows executable: $source -> $destination ($($deployed.Length) bytes, SHA-256 $destinationHash)."

# Deploy to WSL /opt/avrig
$wslDist = "avrig-ai"
$linuxSource = Join-Path $root 'target\release\avrig'
if (Test-Path $linuxSource) {
    Write-Host "Deploying Linux executable to WSL ${wslDist}:/opt/avrig/avrig..." -ForegroundColor Cyan
    
    # Check if a process in WSL is actively running /opt/avrig/avrig
    $checkRunning = wsl.exe -d $wslDist -u root -- bash -c 'pgrep -f "^/opt/avrig/avrig" || true'
    if (-not [string]::IsNullOrWhiteSpace($checkRunning)) {
        Write-Warning "A process is running /opt/avrig/avrig in WSL (PID $checkRunning). Consider stopping it before replacing."
    }

    $deployCmd = @'
set -eu
install -d -m 0755 -o avrig -g avrig /opt/avrig
cp /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api/target/release/avrig /opt/avrig/avrig.tmp
mv -f /opt/avrig/avrig.tmp /opt/avrig/avrig
chown avrig:avrig /opt/avrig/avrig
chmod 0755 /opt/avrig/avrig
sha256sum /opt/avrig/avrig | awk '{print $1}'
'@
    $wslHash = ((& wsl.exe -d $wslDist -u root -- bash -c $deployCmd).Trim() -split '\s+')[0]
    $linuxSourceHash = Get-Sha256 $linuxSource
    if ($wslHash -ne $linuxSourceHash) {
        throw "WSL deployment SHA-256 check failed: source=$linuxSourceHash, destination=$wslHash"
    }
    Write-Output "Deployed Linux executable: /opt/avrig/avrig (SHA-256 $wslHash)."
} else {
    Write-Warning "Linux binary not found at $linuxSource. Run 'make build' or 'make build-linux' first to compile the Linux target."
}

# Configure and ensure Windows Service via NSSM
if ($nssmCmd) {
    Write-Host "`nEnsuring Windows service '$serviceName' via NSSM..." -ForegroundColor Cyan
    $configDir = Join-Path $destinationDir 'config'
    $appArgs = "--config `"$configDir`" run"
    $existingSvc = Get-Service -Name $serviceName -ErrorAction SilentlyContinue

    $isAdmin = ([Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

    # Remove legacy service if present
    if (Get-Service -Name $legacyServiceName -ErrorAction SilentlyContinue) {
        Write-Host "Removing legacy service '$legacyServiceName'..." -ForegroundColor Yellow
        if ($isAdmin) {
            & $nssmCmd.Source stop $legacyServiceName 2>$null
            & $nssmCmd.Source remove $legacyServiceName confirm 2>$null
        } else {
            Start-Process powershell.exe -ArgumentList @("-NoProfile", "-Command", "& `"$($nssmCmd.Source)`" stop $legacyServiceName; & `"$($nssmCmd.Source)`" remove $legacyServiceName confirm") -Verb RunAs -Wait
        }
    }

    if (-not $existingSvc) {
        if ($isAdmin) {
            Write-Host "Installing Windows service '$serviceName'..." -ForegroundColor Cyan
            & $nssmCmd.Source install $serviceName $destination $appArgs
            & $nssmCmd.Source set $serviceName AppDirectory $destinationDir
            & $nssmCmd.Source set $serviceName DisplayName $serviceDisplayName
            & $nssmCmd.Source set $serviceName Description $serviceDescription
            & $nssmCmd.Source set $serviceName Start SERVICE_AUTO_START
            & $nssmCmd.Source start $serviceName
        } else {
            Write-Host "Elevating with UAC to install Windows service '$serviceName'..." -ForegroundColor Cyan
            $elevateScript = @"
& `"$($nssmCmd.Source)`" install $serviceName `"$destination`" $appArgs
& `"$($nssmCmd.Source)`" set $serviceName AppDirectory `"$destinationDir`"
& `"$($nssmCmd.Source)`" set $serviceName DisplayName `"$serviceDisplayName`"
& `"$($nssmCmd.Source)`" set $serviceName Description `"$serviceDescription`"
& `"$($nssmCmd.Source)`" set $serviceName Start SERVICE_AUTO_START
& `"$($nssmCmd.Source)`" start $serviceName
"@
            $proc = Start-Process powershell.exe -ArgumentList @("-NoProfile", "-Command", $elevateScript) -Verb RunAs -PassThru -Wait
        }
    } else {
        # Update and restart existing service
        Write-Host "Restarting Windows service '$serviceName'..." -ForegroundColor Cyan
        if ($isAdmin) {
            & $nssmCmd.Source set $serviceName Application $destination
            & $nssmCmd.Source set $serviceName AppParameters $appArgs
            & $nssmCmd.Source set $serviceName AppDirectory $destinationDir
            & $nssmCmd.Source set $serviceName DisplayName $serviceDisplayName
            & $nssmCmd.Source set $serviceName Description $serviceDescription
            & $nssmCmd.Source restart $serviceName
        } else {
            Start-Process powershell.exe -ArgumentList @("-NoProfile", "-Command", "& `"$($nssmCmd.Source)`" set $serviceName Application `"$destination`"; & `"$($nssmCmd.Source)`" set $serviceName AppParameters $appArgs; & `"$($nssmCmd.Source)`" set $serviceName DisplayName `"$serviceDisplayName`"; & `"$($nssmCmd.Source)`" set $serviceName Description `"$serviceDescription`"; & `"$($nssmCmd.Source)`" restart $serviceName") -Verb RunAs -Wait
        }
    }

    Start-Sleep -Seconds 2
    $svcCheck = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($svcCheck) {
        Write-Output "Windows service '$serviceName' status: $($svcCheck.Status)"
    } else {
        Write-Warning "Windows service '$serviceName' is not yet installed. Please run PowerShell as Administrator once to register the NSSM service."
    }
} else {
    Write-Warning "nssm command not found. Skipping Windows service installation."
}
