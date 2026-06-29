# NAVI installer for Windows (PowerShell)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex"
#
# Environment variables:
#   NAVI_VERSION  — install a specific version (default: latest)
#   NAVI_INSTALL  — installation directory (default: ~/.navi/bin)

$ErrorActionPreference = 'Stop'

# ── Helpers ──────────────────────────────────────────────────────────────────

function Write-Info  { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Cyan }
function Write-Ok    { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Yellow }
function Write-Err   { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Red }

# ── Platform detection ───────────────────────────────────────────────────────

$Arch = if ([Environment]::Is64BitOperatingSystem) { "x64" } else {
    Write-Err "Unsupported architecture: 32-bit Windows is not supported."
    exit 1
}

$Platform = "win32"
$PlatformArch = "${Platform}-${Arch}"

# ── Version resolution ───────────────────────────────────────────────────────

function Get-LatestVersion {
    Write-Info "Fetching latest version..."
    $url = "https://api.github.com/repos/navi-ai-org/navi/releases/latest"
    try {
        $release = Invoke-RestMethod -Uri $url -UseBasicParsing
        $tag = $release.tag_name
        # Strip leading 'v' if present
        if ($tag.StartsWith("v")) { $tag = $tag.Substring(1) }
        return $tag
    } catch {
        Write-Err "Could not determine latest version from GitHub."
        Write-Err "Try setting `$env:NAVI_VERSION` explicitly."
        exit 1
    }
}

# ── Main ─────────────────────────────────────────────────────────────────────

$Version = if ($env:NAVI_VERSION) { $env:NAVI_VERSION } else { Get-LatestVersion }
$InstallDir = if ($env:NAVI_INSTALL) { $env:NAVI_INSTALL } else {
    Join-Path $env:USERPROFILE ".navi\bin"
}

Write-Info "Detected platform: $PlatformArch"
Write-Info "Installing NAVI v$Version"

# Build download URL
$ArchiveName = "navi-${PlatformArch}.zip"
$DownloadUrl = "https://github.com/navi-ai-org/navi/releases/download/v${Version}/${ArchiveName}"

# Create temp directory
$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "navi-install-$([System.Guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    # Download
    $ArchivePath = Join-Path $TmpDir $ArchiveName
    Write-Info "Downloading $DownloadUrl..."

    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
    } catch {
        Write-Err "Download failed. Check that version v${Version} exists:"
        Write-Err "  https://github.com/navi-ai-org/navi/releases"
        exit 1
    }

    # Extract
    Write-Info "Extracting..."
    $ExtractDir = Join-Path $TmpDir "extracted"
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

    # Find the binary
    $Binary = Get-ChildItem -Path $ExtractDir -Filter "navi.exe" -Recurse | Select-Object -First 1

    if (-not $Binary) {
        Write-Err "Could not find navi.exe in the downloaded archive."
        exit 1
    }

    # Install
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $Dest = Join-Path $InstallDir "navi.exe"
    Copy-Item -Path $Binary.FullName -Destination $Dest -Force

    Write-Ok "NAVI v${Version} installed to $Dest"

    # PATH check
    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($CurrentPath -notlike "*$InstallDir*") {
        Write-Warn ""
        Write-Warn "$InstallDir is not in your PATH."
        Write-Warn ""
        Write-Warn "Add it now (restart your terminal after):"

        $AddPath = Read-Host "Add $InstallDir to user PATH? [Y/n]"
        if ($AddPath -eq "" -or $AddPath -eq "Y" -or $AddPath -eq "y") {
            [Environment]::SetEnvironmentVariable("Path", "$CurrentPath;$InstallDir", "User")
            Write-Ok "Added to PATH. Restart your terminal for it to take effect."
        } else {
            Write-Warn "Skipped. Add manually:"
            Write-Warn "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$InstallDir`", 'User')"
        }
    }

    Write-Host ""
    Write-Ok "Run 'navi' to get started."

} finally {
    # Cleanup
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
