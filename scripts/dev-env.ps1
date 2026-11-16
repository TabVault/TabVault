# TabVault build environment.
#
# Why this file exists
# --------------------
# `rusqlite` with `bundled-sqlcipher-vendored-openssl` compiles OpenSSL from
# source, and OpenSSL's Configure step is driven by Perl. On a typical Windows
# developer machine that fails in two avoidable ways:
#
#   1. Perl is missing, or the only Perl on PATH is Git's minimal one, which
#      lacks IPC::Cmd / Locale::Maketext::Simple / Params::Check.
#   2. Some other toolchain (GnuWin32, Cygwin, an old MSYS) sits earlier on PATH
#      and its unquoted `C:\Program Files (x86)\...` entry breaks nmake with
#      "'C:\Program' is not recognized as an internal or external command".
#
# This script fixes both by building a *deliberate* environment instead of
# inheriting whatever the shell happens to have:
#
#   * take the MSVC x64 environment from vcvars64.bat, which also drops the
#     stale toolchains from PATH,
#   * then put the project-local portable Strawberry Perl first.
#
# Usage:
#   . .\scripts\dev-env.ps1          # current shell
#   .\scripts\dev-env.ps1 -Build     # configure, then run a build
#
# The portable Perl lives in `.tools/` and is fetched by
# `scripts/fetch-tools.ps1`, so nothing needs admin rights and nothing is
# installed system-wide.

[CmdletBinding()]
param(
    [switch]$Build,
    [switch]$Release,
    [string]$VcpkgRoot
)

# CRITICAL when dot-sourced: this script must not leak $ErrorActionPreference.
# Setting it to 'Stop' for the duration of the script is fine, but leaving it
# set means the *user's* next `cargo build` treats the first line cargo writes to
# stderr as a terminating error and aborts. We save and restore it in a finally.
$script:PreviousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repoRoot '.tools'

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

try {

# ---------------------------------------------------------------------------
# 1. MSVC x64 environment
# ---------------------------------------------------------------------------
function Get-VcVarsPath {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw "vswhere.exe not found. Install Visual Studio 2022+ with 'Desktop development with C++'."
    }
    $install = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
    if (-not $install) {
        throw "No Visual Studio installation with the C++ toolset was found."
    }
    $vcvars = Join-Path $install 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path $vcvars)) {
        throw "vcvars64.bat not found at $vcvars"
    }
    return $vcvars
}

Write-Step 'Loading MSVC x64 environment (resets PATH to a clean toolchain)'
$vcvars = Get-VcVarsPath

# Run vcvars64 and adopt the resulting environment wholesale. Doing this (rather
# than just prepending paths) is what removes the stale GnuWin32 entry.
$envDump = cmd /c "`"$vcvars`" >nul 2>&1 && set"
if (-not $envDump) { throw "Failed to initialise the MSVC environment from $vcvars" }
foreach ($line in $envDump) {
    $idx = $line.IndexOf('=')
    if ($idx -gt 0) {
        $name = $line.Substring(0, $idx)
        $value = $line.Substring($idx + 1)
        Set-Item -Path "Env:$name" -Value $value
    }
}

# ---------------------------------------------------------------------------
# 2. Portable Strawberry Perl for the OpenSSL build
# ---------------------------------------------------------------------------
$perlBin = Join-Path $toolsRoot 'strawberry-perl\perl\bin'
if (-not (Test-Path (Join-Path $perlBin 'perl.exe'))) {
    Write-Warning "Portable Perl not found at $perlBin. Run scripts\fetch-tools.ps1 first."
} else {
    Write-Step 'Putting portable Strawberry Perl first on PATH'
    $env:PATH = "$perlBin;$env:PATH"
    $env:PERL = Join-Path $perlBin 'perl.exe'
    # Deny-list the toolchains that break OpenSSL's build if they ever reappear.
    $poison = @(
        'C:\Program Files (x86)\GnuWin32\bin',
        'C:\cygwin64\bin',
        'C:\cygwin\bin',
        'C:\msys64\usr\bin'
    )
    $env:PATH = ($env:PATH -split ';' | Where-Object {
        $p = $_.TrimEnd('\')
        $p -ne '' -and ($poison -notcontains $p)
    }) -join ';'

    # Sweep *every other* process env var that references a poisoned directory.
    # On the machine this was written for, `make=C:\Program Files (x86)\GnuWin32\bin`
    # was set globally (a directory, not a binary, unquoted). OpenSSL's generated
    # Makefile expands $(MAKE) as-is and cmd then splits it at the space, producing
    # "'C:\Program' is not recognized as an internal or external command" deep inside
    # the C build. PATH can be clean while `make` still points at GnuWin32, so PATH
    # alone is not enough to sanitize.
    Get-ChildItem env: | ForEach-Object {
        $name = $_.Name
        if ($name -in @('PATH', 'Path', 'PERL', 'SystemRoot', 'ComSpec', 'TEMP', 'TMP', 'CARGO_HOME', 'RUSTUP_HOME')) { return }
        foreach ($bad in $poison) {
            if ($_.Value -like "*$bad*") {
                Write-Host "  removing poisoned env var: $name" -ForegroundColor DarkGray
                Remove-Item -Path "Env:\$name" -ErrorAction SilentlyContinue
                return
            }
        }
    }
    # OpenSSL's MSVC build must use nmake, whatever else the environment holds.
    Remove-Item -Path 'Env:MAKE' -ErrorAction SilentlyContinue
    $env:MAKE = 'nmake'
    Remove-Item -Path 'Env:MAKEFLAGS' -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# 3. Optional vcpkg for anything else
# ---------------------------------------------------------------------------
if ($VcpkgRoot) {
    Write-Step "Adding vcpkg at $VcpkgRoot"
    $env:VCPKG_ROOT = $VcpkgRoot
}

# ---------------------------------------------------------------------------
# 4. Verify the toolchain actually works
# ---------------------------------------------------------------------------
Write-Step 'Verifying toolchain'
$checks = @(
    @{ Name = 'cl';    Cmd = 'cl.exe /? >nul 2>&1' },
    @{ Name = 'nmake'; Cmd = 'nmake /? >nul 2>&1' },
    @{ Name = 'cargo'; Cmd = 'cargo --version >nul 2>&1' }
)
foreach ($c in $checks) {
    cmd /c "$($c.Cmd)" *> $null
    if ($LASTEXITCODE -ne 0) { Write-Warning "  $($c.Name): NOT available" }
    else { Write-Host "  $($c.Name): ok" -ForegroundColor DarkGray }
}
$perlOk = $false
if ($env:PERL) {
    cmd /c "`"$env:PERL`" -MIPC::Cmd -MParams::Check -e `"exit 0`" 2>nul"
    $perlOk = ($LASTEXITCODE -eq 0)
    if ($perlOk) { Write-Host '  perl (with OpenSSL modules): ok' -ForegroundColor DarkGray }
    else { Write-Warning '  perl is present but missing the modules OpenSSL needs' }
}

Write-Host ''
Write-Host 'TabVault build environment ready.' -ForegroundColor Green
Write-Host "  cargo test -p tv-core" -ForegroundColor Gray
if ($Build) {
    $profileArg = if ($Release) { '--release' } else { '' }
    Write-Step "cargo build $profileArg"
    Push-Location $repoRoot
    try { cargo build $profileArg } finally { Pop-Location }
}
}
finally {
    # Restore the caller's preference so dot-sourcing has no side effects beyond
    # the environment variables we deliberately set.
    $ErrorActionPreference = $script:PreviousErrorActionPreference
}
