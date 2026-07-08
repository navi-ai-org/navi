# NAVI installer for Windows (PowerShell)
#
# Primary install method:
#   irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
#
# Or pin a version:
#   $env:NAVI_VERSION = "0.1.2"
#   irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
#
# Environment:
#   NAVI_VERSION  — version or tag (default: latest)
#   NAVI_INSTALL  — install directory (default: %USERPROFILE%\.navi\bin)
#   NAVI_REPO     — GitHub repo (default: navi-ai-org/navi)

$ErrorActionPreference = 'Stop'

function Write-Info { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Green }
function Write-Warn { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Yellow }
function Write-Err  { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Red }

$Repo = if ($env:NAVI_REPO) { $env:NAVI_REPO } else { "navi-ai-org/navi" }

# ── Platform ─────────────────────────────────────────────────────────────────

if (-not [Environment]::Is64BitOperatingSystem) {
    Write-Err "Unsupported architecture: 32-bit Windows is not supported."
    exit 1
}

# Prefer process arch when available (ARM64 Windows).
$ProcArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$Arch = switch -Regex ($ProcArch) {
    'arm64' { 'x64'; Write-Warn "ARM64 Windows: using x64 binary via emulation (no native arm64 build yet)." }
    'x64|x86' { 'x64' }
    default { 'x64' }
}

$PlatformArch = "win32-$Arch"

# ── Version ──────────────────────────────────────────────────────────────────

function Normalize-Version([string]$v) {
    if ($v.StartsWith("v") -or $v.StartsWith("V")) { return $v.Substring(1) }
    return $v
}

function Get-LatestVersion {
    Write-Info "Fetching latest version..."
    $url = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $release = Invoke-RestMethod -Uri $url -UseBasicParsing
        return (Normalize-Version $release.tag_name)
    } catch {
        Write-Err "Could not determine latest version from GitHub."
        Write-Err "Set `$env:NAVI_VERSION explicitly."
        exit 1
    }
}

$Version = if ($env:NAVI_VERSION) { Normalize-Version $env:NAVI_VERSION } else { Get-LatestVersion }
$InstallDir = if ($env:NAVI_INSTALL) { $env:NAVI_INSTALL } else {
    Join-Path $env:USERPROFILE ".navi\bin"
}

Write-Info "Detected platform: $PlatformArch"
Write-Info "Installing NAVI v$Version"

$ArchiveName = "navi-${PlatformArch}.zip"
$DownloadUrl = "https://github.com/$Repo/releases/download/v${Version}/${ArchiveName}"
$SumsUrl = "https://github.com/$Repo/releases/download/v${Version}/SHA256SUMS.txt"

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("navi-install-" + [System.Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    $ArchivePath = Join-Path $TmpDir $ArchiveName
    Write-Info "Downloading $DownloadUrl..."

    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
    } catch {
        Write-Err "Download failed. Check that version v$Version exists:"
        Write-Err "  https://github.com/$Repo/releases"
        exit 1
    }

    # Optional checksum verification
    try {
        $sums = Invoke-WebRequest -Uri $SumsUrl -UseBasicParsing
        $line = ($sums.Content -split "`n" | Where-Object { $_ -match [regex]::Escape($ArchiveName) } | Select-Object -First 1)
        if ($line) {
            $expected = ($line -split "\s+")[0].Trim()
            $sha = [System.Security.Cryptography.SHA256]::Create()
            $bytes = [System.IO.File]::ReadAllBytes($ArchivePath)
            $actual = ([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace "-", "").ToLowerInvariant()
            $sha.Dispose()
            if ($actual -ne $expected.ToLowerInvariant()) {
                Write-Err "Checksum mismatch for $ArchiveName"
                Write-Err "  expected: $expected"
                Write-Err "  actual:   $actual"
                exit 1
            }
            Write-Info "Checksum OK"
        }
    } catch {
        Write-Warn "Could not verify SHA256SUMS.txt; continuing without verify."
    }

    Write-Info "Extracting..."
    $ExtractDir = Join-Path $TmpDir "extracted"
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

    $Binary = Get-ChildItem -Path $ExtractDir -Filter "navi.exe" -Recurse | Select-Object -First 1
    if (-not $Binary) {
        Write-Err "Could not find navi.exe in the downloaded archive."
        exit 1
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $Dest = Join-Path $InstallDir "navi.exe"
    Copy-Item -Path $Binary.FullName -Destination $Dest -Force
    Write-Ok "NAVI v${Version} installed to $Dest"

    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($CurrentPath -notlike "*$InstallDir*") {
        Write-Warn ""
        Write-Warn "$InstallDir is not in your PATH."
        Write-Warn ""

        # Non-interactive (piped iex): auto-add to PATH.
        $interactive = [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
        $add = $true
        if ($interactive) {
            $answer = Read-Host "Add $InstallDir to user PATH? [Y/n]"
            if ($answer -ne "" -and $answer -ne "Y" -and $answer -ne "y") {
                $add = $false
            }
        }

        if ($add) {
            if ([string]::IsNullOrEmpty($CurrentPath)) {
                [Environment]::SetEnvironmentVariable("Path", $InstallDir, "User")
            } else {
                [Environment]::SetEnvironmentVariable("Path", "$CurrentPath;$InstallDir", "User")
            }
            $env:Path = "$env:Path;$InstallDir"
            Write-Ok "Added to PATH. Restart your terminal if 'navi' is not found yet."
        } else {
            Write-Warn "Skipped. Add manually:"
            Write-Warn "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$InstallDir`", 'User')"
        }
    }

    Write-Host ""
    Write-Ok "Run 'navi' to get started."
} finally {
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
