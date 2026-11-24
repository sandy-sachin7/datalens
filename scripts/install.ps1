#Requires -Version 5.1
<#
.SYNOPSIS
    Installs deltalens — Standalone CLI to inspect Delta Lake tables.

.DESCRIPTION
    Downloads the latest pre-built deltalens binary for Windows from GitHub
    Releases, verifies its SHA-256 checksum, and installs it to a directory on your PATH.

.PARAMETER Version
    Specific version to install (e.g. "v1.1.0"). Defaults to latest.

.PARAMETER InstallDir
    Directory to install the binary. Defaults to $env:USERPROFILE\.deltalens\bin

.EXAMPLE
    irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex
#>

[CmdletBinding()]
param(
    [string]$Version     = "",
    [string]$InstallDir  = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Config ───────────────────────────────────────────────────────────────────
$Repo      = "sandy-sachin7/datalens"
$Binary    = "deltalens"
$GithubUrl = "https://github.com/$Repo"
$ApiUrl    = "https://api.github.com/repos/$Repo"

# ── Colours (Windows Terminal / PowerShell 5+) ───────────────────────────────
function Write-Info    { Write-Host "  " -NoNewline; Write-Host "ℹ" -ForegroundColor Cyan -NoNewline; Write-Host "  $args" }
function Write-Success { Write-Host "  " -NoNewline; Write-Host "✔" -ForegroundColor Green -NoNewline; Write-Host "  $args" }
function Write-Warn    { Write-Host "  " -NoNewline; Write-Host "⚠" -ForegroundColor Yellow -NoNewline; Write-Host "  $args" }
function Write-Fail    { Write-Host "  " -NoNewline; Write-Host "✘" -ForegroundColor Red -NoNewline; Write-Host "  $args"; exit 1 }

# ── Banner ───────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "  ██████╗ ███████╗██╗  ████████╗ █████╗ ██╗     ███████╗███╗   ██╗███████╗" -ForegroundColor Cyan
Write-Host "  ██╔══██╗██╔════╝██║  ╚══██╔══╝██╔══██╗██║     ██╔════╝████╗  ██║██╔════╝" -ForegroundColor Cyan
Write-Host "  ██║  ██║█████╗  ██║     ██║   ███████║██║     █████╗  ██╔██╗ ██║███████╗" -ForegroundColor Cyan
Write-Host "  ██║  ██║██╔══╝  ██║     ██║   ██╔══██║██║     ██╔══╝  ██║╚██╗██║╚════██║" -ForegroundColor Cyan
Write-Host "  ██████╔╝███████╗███████╗██║   ██║  ██║███████╗███████╗██║ ╚████║███████║" -ForegroundColor Cyan
Write-Host "  ╚═════╝ ╚══════╝╚══════╝╚═╝   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝  ╚═══╝╚══════╝" -ForegroundColor Cyan
Write-Host ""
Write-Info "Standalone CLI to inspect Delta Lake tables without Spark, JVM, or Python"
Write-Host ""

# ── Detect Architecture ──────────────────────────────────────────────────────
function Get-Architecture {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64" { return "x86_64" }
        "Arm64" {
            Write-Warn "No native Windows ARM64 binary is published yet; installing x86_64 (runs via Windows ARM64 emulation) or build natively via 'cargo install deltalens'."
            return "x86_64"
        }
        default { Write-Fail "Unsupported architecture: $arch. Please build from source: $GithubUrl" }
    }
}

# ── Fetch Latest Version ─────────────────────────────────────────────────────
function Get-LatestVersion {
    if ($Version -ne "") { return $Version }
    Write-Info "Fetching latest release..."
    try {
        $response = Invoke-RestMethod -Uri "$ApiUrl/releases/latest" -Headers @{ "User-Agent" = "deltalens-installer" }
        return $response.tag_name
    } catch {
        Write-Fail "Could not fetch latest version. Check: $GithubUrl/releases`n$_"
    }
}

# ── Resolve Install Directory ─────────────────────────────────────────────────
function Get-InstallDirectory {
    if ($InstallDir -ne "") { return $InstallDir }
    return (Join-Path $env:USERPROFILE ".deltalens\bin")
}

# ── Download & Install ────────────────────────────────────────────────────────
function Install-DeltaLens {
    $arch        = Get-Architecture
    $ver         = Get-LatestVersion
    $target      = "$arch-windows"
    $assetName   = "$Binary-$ver-$target.zip"
    $sha256Name  = "$assetName.sha256"
    $downloadUrl = "$GithubUrl/releases/download/$ver/$assetName"
    $sha256Url   = "$GithubUrl/releases/download/$ver/$sha256Name"

    Write-Info "Version:      $ver"
    Write-Info "Architecture: $arch"

    $dir = Get-InstallDirectory
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    Write-Info "Install dir:  $dir"

    $tmpDir = Join-Path $env:TEMP "deltalens-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

    try {
        $archive = Join-Path $tmpDir $assetName
        $shaFile = Join-Path $tmpDir $sha256Name

        Write-Info "Downloading $Binary $ver for $target..."
        try {
            $ProgressPreference = "SilentlyContinue"
            Invoke-WebRequest -Uri $downloadUrl -OutFile $archive -UseBasicParsing
        } catch {
            Write-Fail "Download failed.`n  URL: $downloadUrl`n  $_`n  Check: $GithubUrl/releases"
        }

        # Strictly enforce SHA-256 checksum verification
        Write-Info "Verifying SHA-256 checksum..."
        try {
            Invoke-WebRequest -Uri $sha256Url -OutFile $shaFile -UseBasicParsing -ErrorAction Stop
        } catch {
            Write-Fail "Failed to download SHA-256 checksum file ($sha256Url). Aborting installation."
        }

        $expected = ((Get-Content $shaFile -Raw).Trim() -split '\s+')[0].ToLower()
        if ([string]::IsNullOrWhiteSpace($expected)) {
            Write-Fail "Checksum file is empty or malformed. Aborting installation."
        }
        $actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) {
            Write-Fail "SHA-256 checksum mismatch! Expected: $expected  Got: $actual. Aborting installation."
        }
        Write-Success "SHA-256 checksum verified"

        # Extract
        Expand-Archive -Path $archive -DestinationPath $tmpDir -Force

        $exe = Get-ChildItem -Path $tmpDir -Recurse -Filter "$Binary.exe" | Select-Object -First 1
        if (-not $exe) {
            Write-Fail "Binary not found in archive. Please file an issue: $GithubUrl/issues"
        }

        $dest = Join-Path $dir "$Binary.exe"
        Copy-Item -Path $exe.FullName -Destination $dest -Force

        # Add to user PATH
        $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
        if ($userPath -notlike "*$dir*") {
            [Environment]::SetEnvironmentVariable("PATH", "$dir;$userPath", "User")
            Write-Success "Added $dir to your user PATH"
            Write-Warn "Restart your terminal for PATH changes to take effect."
        }
        $env:PATH = "$dir;$env:PATH"

        try {
            $installedVer = & $dest --version 2>&1 | Select-Object -First 1
            Write-Success "Installed: $installedVer"
        } catch {
            Write-Warn "Binary installed but could not verify. Try: $dest --version"
        }

        Write-Host ""
        Write-Success "$Binary is ready!"
        Write-Host ""
        Write-Host "  Quick start:" -ForegroundColor White
        Write-Host "    $Binary inspect C:\path\to\delta\table" -ForegroundColor Yellow
        Write-Host "    $Binary --help" -ForegroundColor Yellow
        Write-Host ""
        Write-Host "  Docs & source: $GithubUrl" -ForegroundColor Cyan
        Write-Host ""
    } finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
    }
}

Install-DeltaLens
