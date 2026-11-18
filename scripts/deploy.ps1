[CmdletBinding()]
param(
    [ValidateSet('all', 'windows', 'linux')]
    [string]$Target = 'all'
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$destinationDir = 'D:\avrig'

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

if ($Target -in @('all', 'windows')) {
    $source = Join-Path $root 'target\release\avrig.exe'
    $destination = Join-Path $destinationDir 'avrig.exe'
    $staged = Join-Path $destinationDir ("avrig.exe.deploy-$([guid]::NewGuid().ToString('N')).tmp")

    if (-not (Test-Path $source)) {
        throw "Windows release executable not found: $source. Run 'make build' or 'make build-windows' first."
    }

    $serviceName = 'avrig'
    $legacyServiceName = 'avrig-watcher'
    $serviceDisplayName = 'avrig Windows Runtime Node'
    $serviceDescription = 'Unified cross-platform avrig runtime node hosting directory ingestion watcher, hardware-accelerated neural reranker, and AI Gateway.'
    $nssmCmd = Get-Command nssm -ErrorAction SilentlyContinue

    # If legacy or current NSSM service exists, stop it before replacing the binary
    $isAdmin = ([Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($nssmCmd) {
        foreach ($name in @($serviceName, $legacyServiceName)) {
            $svc = Get-Service -Name $name -ErrorAction SilentlyContinue
            if ($svc -and $svc.Status -eq 'Running') {
                Write-Host "Stopping Windows service $name..." -ForegroundColor Cyan
                if ($isAdmin) {
                    & $nssmCmd.Source stop $name 2>$null
                } else {
                    Start-Process powershell.exe -ArgumentList @("-NoProfile", "-Command", "& `"$($nssmCmd.Source)`" stop $name") -Verb RunAs -Wait
                }
                Start-Sleep -Seconds 2
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
        if (Test-Path $destination) {
            Remove-Item $destination -Force
        }
        Move-Item -Path $staged -Destination $destination -Force
    }
    finally {
        if (Test-Path $staged) {
            Remove-Item $staged -Force
        }
    }

    $deployed = Get-Item $destination
    $built = Get-Item $source

    $sourceHash = Get-Sha256 $source
    $destinationHash = Get-Sha256 $destination
    if ($sourceHash -ne $destinationHash) {
        throw "Deployment SHA-256 check failed: source=$sourceHash, destination=$destinationHash"
    }

    Write-Output "Deployed Windows executable: $source -> $destination ($($deployed.Length) bytes, SHA-256 $destinationHash)."

    # Synchronize config files into destination config directory
    $destinationConfigDir = Join-Path $destinationDir 'config'
    New-Item -ItemType Directory -Path $destinationConfigDir -Force | Out-Null
    $sourceConfigDir = Join-Path $root 'config'
    if (Test-Path $sourceConfigDir) {
        # Ensure gateway.toml is deployed
        $srcGatewayToml = Join-Path $sourceConfigDir 'gateway.toml'
        if (Test-Path $srcGatewayToml) {
            Copy-Item -Path $srcGatewayToml -Destination (Join-Path $destinationConfigDir 'gateway.toml') -Force
            Write-Host "Synchronized gateway.toml to $destinationConfigDir" -ForegroundColor Cyan
        }
    }

    # Ensure D:\avrig\config\config.toml includes the 'gateway' role
    $destConfigToml = Join-Path $destinationConfigDir 'config.toml'
    if (Test-Path $destConfigToml) {
        $cfgContent = Get-Content $destConfigToml -Raw
        if ($cfgContent -match 'roles\s*=\s*\[(.*?)\]') {
            $rolesList = $matches[1]
            if ($rolesList -notmatch '"gateway"') {
                $newRoles = $rolesList.Trim()
                if ($newRoles.Length -gt 0 -and -not $newRoles.EndsWith(',')) {
                    $newRoles += ', '
                }
                $newRoles += '"gateway"'
                $cfgContent = $cfgContent -replace 'roles\s*=\s*\[.*?\]', "roles = [$newRoles]"
                Set-Content -Path $destConfigToml -Value $cfgContent -Force
                Write-Host "Updated $destConfigToml to include 'gateway' role: roles = [$newRoles]" -ForegroundColor Cyan
            }
        }
    }
}

# Deploy to WSL /opt/avrig
if ($Target -in @('all', 'linux')) {
    $wslDist = "avrig-ai"
    $linuxSource = Join-Path $root 'target\release\avrig'
    if (Test-Path $linuxSource) {
        Write-Host "Deploying Linux executable to WSL ${wslDist}:/opt/avrig/avrig..." -ForegroundColor Cyan
        
        # Check if avrig.service in WSL is active, stop it before deployment
        $svcActive = wsl.exe -d $wslDist -u root -- bash -c 'systemctl is-active avrig.service || true'
        if ($svcActive.Trim() -eq 'active') {
            Write-Host "Stopping avrig.service in WSL..." -ForegroundColor Cyan
            wsl.exe -d $wslDist -u root -- systemctl stop avrig.service
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

        # Restart avrig.service in WSL
        Write-Host "Starting avrig.service in WSL..." -ForegroundColor Cyan
        wsl.exe -d $wslDist -u root -- systemctl start avrig.service
    } else {
        Write-Warning "Linux binary not found at $linuxSource. Run 'make build' or 'make build-linux' first to compile the Linux target."
    }
}

# Configure and ensure Windows Service via NSSM
if ($Target -in @('all', 'windows')) {
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
}

