# Registers TabVault's native-messaging host with Chrome, Edge and Firefox.
#
# The browsers discover the host two ways on Windows:
#   Chrome:  HKCU\Software\Google\Chrome\NativeMessagingHosts\com.tabvault.host
#   Edge:    HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.tabvault.host
#   Firefox: HKCU\Software\Mozilla\NativeMessagingHosts\com.tabvault.host
# each set to the full path of a JSON manifest that points at the host binary.
#
# This script:
#   1. runs tabvault-host.exe --selftest to prove the vault opens,
#   2. writes the manifest(s) next to the host binary,
#   3. writes the registry keys (HKCU by default, HKLM with -AllUsers),
#   4. optionally removes them with -Uninstall.
#
# The manifest is one shared file used by all three browsers: each browser
# ignores the allow-list keys it does not understand (verified: Chrome reads
# only allowed_origins, Firefox only allowed_extensions, Edge reads
# allowed_origins), so a single manifest keeps registration simple.

[CmdletBinding()]
param(
    # Directory containing tabvault-host.exe. Defaults to this script's location,
    # which is where the installer places it.
    [string]$AppDir,
    # Register for all users (HKLM) instead of HKCU. Requires elevation.
    [switch]$AllUsers,
    # Remove the host registration instead of creating it.
    [switch]$Uninstall,
    # Chrome extension id (derived from the pinned public key in the manifest).
    [string]$ChromeId = 'fbeikljjdhlgipmpgeohjigbgdmnkeae',
    # Firefox add-on id (browser_specific_settings.gecko.id).
    [string]$FirefoxId = 'tabvault@tabvault'
)

$ErrorActionPreference = 'Stop'

$HostName = 'com.tabvault.host'

if (-not $AppDir) {
    $AppDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$AppDir = (Resolve-Path $AppDir).Path
$HostExe = Join-Path $AppDir 'tabvault-host.exe'
$ManifestPath = Join-Path $AppDir "$HostName.json"

# ---------------------------------------------------------------------------
# Registry helpers
# ---------------------------------------------------------------------------
function Set-NativeHostKey {
    param([string]$BrowserKey)
    $path = if ($AllUsers) {
        "HKLM:\SOFTWARE\$BrowserKey\$HostName"
    } else {
        "HKCU:\SOFTWARE\$BrowserKey\$HostName"
    }
    New-Item -Path $path -Force | Out-Null
    Set-ItemProperty -Path $path -Name '(default)' -Value $ManifestPath | Out-Null
    Write-Host "  registered HK$(if ($AllUsers) {'LM'} else {'CU'}): $BrowserKey\$HostName" -ForegroundColor DarkGray
}

function Remove-NativeHostKey {
    param([string]$BrowserKey)
    $path = if ($AllUsers) {
        "HKLM:\SOFTWARE\$BrowserKey\$HostName"
    } else {
        "HKCU:\SOFTWARE\$BrowserKey\$HostName"
    }
    if (Test-Path $path) {
        Remove-Item -Path $path -Recurse -Force
        Write-Host "  removed $path" -ForegroundColor DarkGray
    }
}

$BrowserKeys = @(
    'Google\Chrome\NativeMessagingHosts',
    'Microsoft\Edge\NativeMessagingHosts',
    'Mozilla\NativeMessagingHosts'
)

if ($Uninstall) {
    Write-Host 'Unregistering TabVault native host...' -ForegroundColor Cyan
    foreach ($key in $BrowserKeys) {
        Remove-NativeHostKey -BrowserKey $key
    }
    # The manifest is intentionally left in place so the vault stays importable;
    # delete it manually if desired:
    Write-Host "  manifest left at $ManifestPath (delete manually if wanted)" -ForegroundColor DarkGray
    Write-Host 'Done.' -ForegroundColor Green
    exit 0
}

# ---------------------------------------------------------------------------
# 1. Sanity check: the host binary must exist and pass --selftest.
# ---------------------------------------------------------------------------
Write-Host 'Validating tabvault-host.exe...' -ForegroundColor Cyan
if (-not (Test-Path $HostExe)) {
    throw "tabvault-host.exe not found in $AppDir. Point -AppDir at the install directory."
}
# Capture stdout and stderr separately: PowerShell turns native stderr into
# error records, which would abort the script even when the tool succeeded.
$selfTestOut = & $HostExe --selftest 2>$null
$selfTestExit = $LASTEXITCODE
if ($selfTestExit -ne 0) {
    $selfTestErr = & $HostExe --selftest 2>&1 | Out-String
    throw "tabvault-host.exe --selftest failed (exit $selfTestExit): $selfTestErr"
}
Write-Host "  $selfTestOut" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# 2. Write the manifest.
# ---------------------------------------------------------------------------
Write-Host 'Writing native messaging host manifest...' -ForegroundColor Cyan
$manifest = [ordered]@{
    name                = $HostName
    description         = 'TabVault native messaging host: browser extension to local SQLite vault'
    path                = $HostExe
    type                = 'stdio'
    # Chrome/Edge: exactly these origins may connect.
    allowed_origins     = @(
        "chrome-extension://$ChromeId/"
    )
    # Firefox: exactly these add-on ids may connect.
    allowed_extensions  = @($FirefoxId)
}
$json = $manifest | ConvertTo-Json -Depth 4
# ConvertTo-Json mangles nothing here, but ensure trailing newline and no BOM.
$utf8NoBom = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText($ManifestPath, $json + "`n", $utf8NoBom)
Write-Host "  $ManifestPath" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# 3. Register with the browsers.
# ---------------------------------------------------------------------------
Write-Host 'Registering native messaging host...' -ForegroundColor Cyan
if ($AllUsers) {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw '-AllUsers requires an elevated PowerShell session (run as administrator).'
    }
}
foreach ($key in $BrowserKeys) {
    Set-NativeHostKey -BrowserKey $key
}

Write-Host ''
Write-Host 'TabVault native host registered.' -ForegroundColor Green
Write-Host "  Host name: $HostName" -ForegroundColor Gray
Write-Host "  Manifest:  $ManifestPath" -ForegroundColor Gray
Write-Host "  Chrome id: $ChromeId" -ForegroundColor Gray
Write-Host "  Firefox id: $FirefoxId" -ForegroundColor Gray
Write-Host ''
Write-Host 'To verify, open Chrome/Edge/Firefox with the TabVault extension loaded' -ForegroundColor Gray
Write-Host 'and check the popup shows "host connected".' -ForegroundColor Gray
