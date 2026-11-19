[CmdletBinding()]
param(
    [ValidateSet('all', 'windows', 'linux')]
    [string]$Target = 'all'
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$destinationDir = 'D:\avrig'

# The answer model every node must carry. Patched into each node's own
# server.toml; the repository template is never copied, because it binds 0.0.0.0.
$serverAnswerModel = 'opencode/big-pickle'

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

function Get-ServerTomlValue([string]$Content, [string]$Key) {
    $pattern = '(?m)^[ \t]*' + [regex]::Escape($Key) + '[ \t]*=[ \t]*(?<value>[^\r\n]*)'
    $match = [regex]::Match($Content, $pattern)
    if (-not $match.Success) {
        return $null
    }
    return $match.Groups['value'].Value.Trim().Trim('"')
}

<#
.SYNOPSIS
Returns server config text with only the answer_model line changed.

.DESCRIPTION
Every other line, including the host binding, is carried through untouched. The
function refuses to return text whose host binding differs from the input, so a
loopback deployment can never silently turn into a public listener.
#>
function Update-ServerTomlText {
    param(
        [Parameter(Mandatory)][string]$Content,
        [Parameter(Mandatory)][string]$Model,
        [Parameter(Mandatory)][string]$Description
    )

    $hostBefore = Get-ServerTomlValue -Content $Content -Key 'host'
    if (-not $hostBefore) {
        throw "The $Description server config has no host setting. Refusing to patch it."
    }

    $replacement = 'answer_model = "' + $Model + '"'
    # Capture the leading indent and the line ending separately, so a CRLF file
    # stays CRLF instead of quietly gaining mixed endings.
    $pattern = '(?m)^(?<lead>[ \t]*)answer_model[ \t]*=[ \t]*[^\r\n]*(?<eol>\r?)$'

    if ([regex]::IsMatch($Content, $pattern)) {
        $evaluator = [System.Text.RegularExpressions.MatchEvaluator] {
            param($match)
            return $match.Groups['lead'].Value + $replacement + $match.Groups['eol'].Value
        }
        $updated = [regex]::Replace($Content, $pattern, $evaluator)
    } else {
        # Add the key to the existing [server] section; do not invent a section.
        $header = [regex]::Match($Content, '(?m)^(?<lead>[ \t]*)\[server\][ \t]*\r?\n')
        if (-not $header.Success) {
            throw "The $Description server config has no [server] section. Refusing to patch it."
        }
        $newline = if ($Content.Contains("`r`n")) { "`r`n" } else { "`n" }
        $updated = $Content.Insert($header.Index + $header.Length, $replacement + $newline)
    }

    $hostAfter = Get-ServerTomlValue -Content $updated -Key 'host'
    if ($hostAfter -cne $hostBefore) {
        throw "Refusing to write the $Description server config: the host binding would change from '$hostBefore' to '$hostAfter'."
    }
    if ((Get-ServerTomlValue -Content $updated -Key 'answer_model') -cne $Model) {
        throw "Failed to set answer_model in the $Description server config."
    }

    return $updated
}

<#
.SYNOPSIS
Patches answer_model in a local server.toml, preserving everything else.

.DESCRIPTION
Fails loudly when the file is missing rather than creating one blind. Keeps the
file's encoding and records the previous content beside it before writing.
#>
function Set-ServerTomlAnswerModel {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Model,
        [Parameter(Mandatory)][string]$Description
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "$Description server config not found: $Path. Create it deliberately; the deploy script will not invent one."
    }

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $hasBom = ($bytes.Length -ge 3) -and ($bytes[0] -eq 0xEF) -and ($bytes[1] -eq 0xBB) -and ($bytes[2] -eq 0xBF)
    $content = [System.IO.File]::ReadAllText($Path)
    $updated = Update-ServerTomlText -Content $content -Model $Model -Description $Description

    if ($updated -ceq $content) {
        Write-Host "$Description server config already carries answer_model = `"$Model`" ($Path)" -ForegroundColor DarkCyan
        return
    }

    $backup = "$Path.bak-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'))-deploy"
    Copy-Item -LiteralPath $Path -Destination $backup -Force
    [System.IO.File]::WriteAllText($Path, $updated, (New-Object System.Text.UTF8Encoding($hasBom)))
    Write-Host "$Description server config answer_model set to `"$Model`" ($Path, previous file at $backup)" -ForegroundColor Cyan
}

<#
.SYNOPSIS
Prints the effective host and answer_model for one node.
#>
function Show-ServerConfig {
    param(
        [Parameter(Mandatory)][string]$Description,
        [Parameter(Mandatory)][string]$Content
    )

    $hostValue = Get-ServerTomlValue -Content $Content -Key 'host'
    $modelValue = Get-ServerTomlValue -Content $Content -Key 'answer_model'
    Write-Output "$($Description): host=$hostValue answer_model=$modelValue"
    if ($hostValue -ne '127.0.0.1') {
        Write-Warning "$Description is not bound to 127.0.0.1 (host=$hostValue)."
    }
    if ($modelValue -ne $serverAnswerModel) {
        Write-Warning "$Description answer_model is '$modelValue', expected '$serverAnswerModel'."
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

    # Ensure server.toml carries the answer model. The file is created once with
    # an explicit loopback binding, never copied from the repository template,
    # which binds 0.0.0.0.
    $destServerToml = Join-Path $destinationConfigDir 'server.toml'
    if (-not (Test-Path -LiteralPath $destServerToml)) {
        $windowsServerToml = @(
            '[server]'
            'host = "127.0.0.1"'
            'port = 8000'
            'upload_dir = "data/uploads"'
            "answer_model = `"$serverAnswerModel`""
            'search_threshold = 0.1'
            ''
        ) -join "`n"
        [System.IO.File]::WriteAllText($destServerToml, $windowsServerToml, (New-Object System.Text.UTF8Encoding($false)))
        Write-Host "Created Windows server config with loopback binding: $destServerToml" -ForegroundColor Cyan
    }
    Set-ServerTomlAnswerModel -Path $destServerToml -Model $serverAnswerModel -Description 'Windows'
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

        # Patch answer_model in the node's own server.toml before the service
        # starts, so the node comes up on the new model rather than the old one.
        # Only that one line changes; the loopback binding, port, upload_dir, and
        # search_threshold stay exactly as the node has them. The repository
        # template is never copied, and a missing file is an error rather than a
        # silent create. The script arrives on stdin because wsl.exe mangles
        # multi-line arguments that contain quotes, and it reports on stdout so
        # the deploy does not trip over WSL's own stderr chatter.
        $linuxServerToml = '/opt/avrig/config/server.toml'
        $patchScript = @'
set -eu
path="$1"
model="$2"
if [ ! -f "$path" ]; then
  echo "DEPLOY_ERROR missing:$path"
  exit 3
fi
# Reads one key from the [server] section: strips the key, any trailing CR left
# by a Windows editor, and the surrounding quotes.
get_val() {
  sed -nE "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$path" | head -n 1 | tr -d '\r' | sed -E 's/^"(.*)"$/\1/'
}
host_before=$(get_val host)
if [ -z "$host_before" ]; then
  echo "DEPLOY_ERROR no-host:$path"
  exit 4
fi
if ! grep -qE '^[[:space:]]*answer_model[[:space:]]*=' "$path"; then
  echo "DEPLOY_ERROR no-answer-model:$path"
  exit 5
fi
model_before=$(get_val answer_model)
changed=no
if [ "$model_before" != "$model" ]; then
  changed=yes
  backup="$path.bak-$(date -u +%Y%m%dT%H%M%SZ)-deploy"
  cp -p "$path" "$backup"
  # [^[:cntrl:]] stops before a trailing CR, so a CRLF file stays CRLF.
  sed -E "s|^([[:space:]]*)answer_model[[:space:]]*=[^[:cntrl:]]*|\1answer_model = \"$model\"|" "$path" > "$path.deploy-tmp"
  chown --reference="$path" "$path.deploy-tmp" 2>/dev/null || true
  chmod --reference="$path" "$path.deploy-tmp" 2>/dev/null || true
  mv -f "$path.deploy-tmp" "$path"
fi
host_after=$(get_val host)
model_after=$(get_val answer_model)
if [ "$host_before" != "$host_after" ]; then
  echo "DEPLOY_ERROR host-changed:$host_before->$host_after"
  exit 6
fi
if [ "$model_after" != "$model" ]; then
  echo "DEPLOY_ERROR model-not-set:$model_after"
  exit 7
fi
echo "DEPLOY_OK host=$host_after model=$model_after changed=$changed"
'@
        $patchOutput = @($patchScript | & wsl.exe -d $wslDist -u root -- bash -s -- $linuxServerToml $serverAnswerModel)
        $patchExit = $LASTEXITCODE
        $patchOutput = @($patchOutput | ForEach-Object { "$_".Trim() } | Where-Object { $_ -ne '' })
        if ($patchExit -ne 0) {
            throw "Failed to patch $linuxServerToml in WSL ${wslDist} (exit $patchExit): $($patchOutput -join ' ')"
        }
        Write-Host "Linux server config patched: $($patchOutput -join ' ') ($linuxServerToml)" -ForegroundColor Cyan

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

# Report the effective answer model and binding for every node this run touched,
# read back from disk rather than from memory, so the printed values are real.
Write-Host "`nEffective server configuration:" -ForegroundColor Cyan
if ($Target -in @('all', 'windows')) {
    $reportPath = Join-Path (Join-Path $destinationDir 'config') 'server.toml'
    if (Test-Path -LiteralPath $reportPath) {
        Show-ServerConfig -Description 'Windows' -Content ([System.IO.File]::ReadAllText($reportPath))
        Write-Output "  file: $reportPath"
    } else {
        Write-Warning "Windows server config is missing: $reportPath"
    }
}
if ($Target -in @('all', 'linux')) {
    $linuxReportScript = @'
if [ -f /opt/avrig/config/server.toml ]; then
  cat /opt/avrig/config/server.toml
else
  echo DEPLOY_MISSING
fi
'@
    $linuxContent = ($linuxReportScript | & wsl.exe -d 'avrig-ai' -u root -- bash -s) -join "`n"
    $reportExit = $LASTEXITCODE
    if ($reportExit -ne 0 -or $linuxContent -match 'DEPLOY_MISSING' -or [string]::IsNullOrWhiteSpace($linuxContent)) {
        Write-Warning 'Linux server config is missing or unreadable: /opt/avrig/config/server.toml'
    } else {
        Show-ServerConfig -Description 'Linux' -Content $linuxContent
        Write-Output '  file: /opt/avrig/config/server.toml (avrig-ai)'
    }
}

