#Requires -Version 5.1
<#
.SYNOPSIS
    Installs deltalens — Zero-dependency Delta Lake observability CLI.

.DESCRIPTION
    Downloads the latest pre-built deltalens binary for Windows from GitHub
    Releases and installs it to a directory on your PATH.

.PARAMETER Version
    Specific version to install (e.g. "v0.1.0"). Defaults to latest.

.PARAMETER InstallDir
    Directory to install the binary. Defaults to $env:USERPROFILE\.deltalens\bin

.EXAMPLE
    irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex

.EXAMPLE
    irm https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.ps1 | iex; Install-DeltaLens -Version v0.2.0
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
Write-Info "Zero-dependency Delta Lake observability CLI — written in Rust"
Write-Host ""

# ── Detect Architecture ──────────────────────────────────────────────────────
function Get-Architecture {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64"   { return "x86_64" }
        "Arm64" { return "aarch64" }
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
    $dir = Join-Path $env:USERPROFILE ".deltalens\bin"
    return $dir
}

# ── Download & Install ────────────────────────────────────────────────────────
function Install-DeltaLens {
    $arch       = Get-Architecture
    $ver        = Get-LatestVersion
    $target     = "$arch-windows"
    $assetName  = "$Binary-$ver-$target.zip"
    $sha256Name = "$assetName.sha256"
    $downloadUrl = "$GithubUrl/releases/download/$ver/$assetName"
    $sha256Url   = "$GithubUrl/releases/download/$ver/$sha256Name"

    Write-Info "Version:      $ver"
    Write-Info "Architecture: $arch"

    $dir = Get-InstallDirectory
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    Write-Info "Install dir:  $dir"

    # Download archive
    $tmpDir  = Join-Path $env:TEMP "deltalens-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
    $archive = Join-Path $tmpDir $assetName

    Write-Info "Downloading $Binary $ver for $target..."
    try {
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest -Uri $downloadUrl -OutFile $archive -UseBasicParsing
    } catch {
        Write-Fail "Download failed.`n  URL: $downloadUrl`n  $_ `n  Check: $GithubUrl/releases"
    }

    # Verify checksum
    try {
        $shaFile = Join-Path $tmpDir $sha256Name
        Invoke-WebRequest -Uri $sha256Url -OutFile $shaFile -UseBasicParsing -ErrorAction Stop
        $expected = (Get-Content $shaFile -Raw).Split(" ")[0].Trim().ToLower()
        $actual   = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) {
            Write-Warn "Checksum mismatch! Expected: $expected  Got: $actual"
            Write-Warn "Proceeding anyway — consider re-running the installer."
        } else {
            Write-Success "SHA256 checksum verified"
        }
    } catch {
        Write-Warn "No checksum file found, skipping verification."
    }

    # Extract
    Expand-Archive -Path $archive -DestinationPath $tmpDir -Force

    # Find the exe inside extracted folder
    $exe = Get-ChildItem -Path $tmpDir -Recurse -Filter "$Binary.exe" | Select-Object -First 1
    if (-not $exe) {
        Write-Fail "Binary not found in archive. Please file an issue: $GithubUrl/issues"
    }

    # Copy to install dir
    $dest = Join-Path $dir "$Binary.exe"
    Copy-Item -Path $exe.FullName -Destination $dest -Force

    # Cleanup temp
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

    # ── Add to PATH ───────────────────────────────────────────────────────────
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($userPath -notlike "*$dir*") {
        [Environment]::SetEnvironmentVariable("PATH", "$dir;$userPath", "User")
        Write-Success "Added $dir to your user PATH"
        Write-Warn "Restart your terminal for PATH changes to take effect."
    }
    $env:PATH = "$dir;$env:PATH"

    # ── Verify ────────────────────────────────────────────────────────────────
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
}

Install-DeltaLens
