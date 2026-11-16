# Fetches the build-only tools TabVault needs into `.tools/`, without admin
# rights and without touching the system installation.
#
# Right now that is one thing: a portable Strawberry Perl, which the vendored
# OpenSSL build inside `rusqlite/bundled-sqlcipher-vendored-openssl` needs.
# Shipping a static binary (rather than linking a system libcrypto DLL) is the
# goal, so this step is part of building rather than part of installing.

[CmdletBinding()]
param(
    [string]$PerlVersion = '5.42.3.1',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repoRoot '.tools'
New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null

# ---------------------------------------------------------------------------
# Portable Strawberry Perl
# ---------------------------------------------------------------------------
$perlDir = Join-Path $toolsRoot 'strawberry-perl'
$perlExe = Join-Path $perlDir 'perl\bin\perl.exe'

if ((Test-Path $perlExe) -and -not $Force) {
    Write-Host "Portable Perl already present: $perlExe" -ForegroundColor Green
    exit 0
}

$archiveName = "strawberry-perl-$PerlVersion-64bit-portable.zip"
$archivePath = Join-Path $toolsRoot $archiveName
$url = "https://github.com/StrawberryPerl/Perl-Dist-Strawberry/releases/download/" +
       "SP_$($PerlVersion -replace '\.','')" + "_64bit/$archiveName"

Write-Host "Downloading portable Perl $PerlVersion" -ForegroundColor Cyan
Write-Host "  $url" -ForegroundColor DarkGray

if (-not (Test-Path $archivePath) -or $Force) {
    # curl.exe ships with Windows 10+ and, unlike Invoke-WebRequest, has no
    # progress-bar overhead that makes a 300 MB download crawl under a console.
    & curl.exe -sSL --fail --retry 3 -o $archivePath $url
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to download $url. Download it manually into $archivePath and re-run."
    }
}

Write-Host 'Extracting...' -ForegroundColor Cyan
if (Test-Path $perlDir) { Remove-Item -Recurse -Force $perlDir }
Expand-Archive -Path $archivePath -DestinationPath $perlDir -Force

if (-not (Test-Path $perlExe)) { throw "Extraction did not produce $perlExe" }

# Verify the modules the OpenSSL Configure step requires, so a failure surfaces
# here rather than twenty minutes into a C build.
Write-Host 'Verifying Perl modules needed by OpenSSL' -ForegroundColor Cyan
foreach ($mod in 'IPC::Cmd', 'Locale::Maketext::Simple', 'Params::Check') {
    & $perlExe "-M$mod" -e 'exit 0' 2>$null
    if ($LASTEXITCODE -ne 0) { throw "Portable Perl is missing $mod" }
    Write-Host "  $mod ok" -ForegroundColor DarkGray
}

# The archive is only needed for a re-extract; keeping it doubles the disk cost.
Remove-Item -Force $archivePath -ErrorAction SilentlyContinue

Write-Host ''
Write-Host 'Portable Perl ready.' -ForegroundColor Green
Write-Host 'Now run: . .\scripts\dev-env.ps1' -ForegroundColor Gray
