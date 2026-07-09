# NAVI installer for Windows (PowerShell)
#
# Primary install method:
#   irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
#
# Pin a version:
#   $env:NAVI_VERSION = "0.2.0"
#   irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
#
# Security:
#   • HTTPS only
#   • Archive SHA-256 MUST match release SHA256SUMS.txt (hard fail)
#   • Zip may only contain a single root file: navi.exe
#   • Install under user profile by default (%USERPROFILE%\.navi\bin)
#
# Environment:
#   NAVI_VERSION  — version or tag (default: latest)
#   NAVI_INSTALL  — install directory
#   NAVI_REPO     — GitHub repo (default: navi-ai-org/navi)

$ErrorActionPreference = 'Stop'

function Write-Info { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Green }
function Write-Warn { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Yellow }
function Write-Err  { param([string]$Msg) Write-Host "[navi] $Msg" -ForegroundColor Red }

$Repo = if ($env:NAVI_REPO) { $env:NAVI_REPO } else { "navi-ai-org/navi" }

if (-not [Environment]::Is64BitOperatingSystem) {
    Write-Err "Unsupported architecture: 32-bit Windows is not supported."
    exit 1
}

$ProcArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$Arch = switch -Regex ($ProcArch) {
    'arm64' { 'x64'; Write-Warn "ARM64 Windows: using x64 binary via emulation (no native arm64 build yet)." }
    'x64|x86' { 'x64' }
    default { 'x64' }
}

$PlatformArch = "win32-$Arch"

function Normalize-Version([string]$v) {
    if ($v.StartsWith("v") -or $v.StartsWith("V")) { $v = $v.Substring(1) }
    if ($v -notmatch '^[A-Za-z0-9._-]+$') {
        Write-Err "Invalid version string: $v"
        exit 1
    }
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

function Get-Sha256Hex([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.IO.File]::ReadAllBytes($Path)
        return ([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace "-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
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

if (-not $DownloadUrl.StartsWith("https://") -or -not $SumsUrl.StartsWith("https://")) {
    Write-Err "Refusing non-HTTPS download URL."
    exit 1
}

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("navi-install-" + [System.Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    $ArchivePath = Join-Path $TmpDir $ArchiveName
    $SumsPath = Join-Path $TmpDir "SHA256SUMS.txt"

    Write-Info "Downloading $DownloadUrl..."
    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
    } catch {
        Write-Err "Download failed. Check that version v$Version exists:"
        Write-Err "  https://github.com/$Repo/releases"
        exit 1
    }

    Write-Info "Downloading SHA256SUMS.txt..."
    try {
        Invoke-WebRequest -Uri $SumsUrl -OutFile $SumsPath -UseBasicParsing
    } catch {
        Write-Err "Failed to download SHA256SUMS.txt — refusing to install."
        exit 1
    }

    $sumsText = Get-Content -Raw -Path $SumsPath
    $line = ($sumsText -split "`n" | Where-Object {
        $_ -match ("\s" + [regex]::Escape($ArchiveName) + "\s*$") -or
        $_ -match ("\s\*" + [regex]::Escape($ArchiveName) + "\s*$")
    } | Select-Object -First 1)

    if (-not $line) {
        Write-Err "No SHA-256 entry for $ArchiveName in SHA256SUMS.txt"
        Write-Err "Refusing to install."
        exit 1
    }

    $expected = (($line -split "\s+")[0]).Trim().ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') {
        Write-Err "Malformed checksum for ${ArchiveName}: $expected"
        exit 1
    }

    $actual = Get-Sha256Hex $ArchivePath
    if ($actual -ne $expected) {
        Write-Err "Checksum mismatch for $ArchiveName"
        Write-Err "  expected: $expected"
        Write-Err "  actual:   $actual"
        Write-Err "The download may be corrupt or tampered with. Aborting."
        exit 1
    }
    Write-Info ("SHA-256 OK (" + $actual.Substring(0, 12) + "…)")

    Write-Info "Extracting (single-file navi.exe)..."
    $ExtractDir = Join-Path $TmpDir "extracted"
    New-Item -ItemType Directory -Path $ExtractDir -Force | Out-Null

    # Validate zip members before extract (zip-slip / multi-file rejection).
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @($zip.Entries | Where-Object { -not [string]::IsNullOrEmpty($_.Name) -or $_.FullName.EndsWith("/") })
        $files = @($zip.Entries | Where-Object { -not $_.FullName.EndsWith("/") })
        if ($files.Count -ne 1) {
            Write-Err "Archive must contain exactly one file (found $($files.Count))."
            exit 1
        }
        $entry = $files[0]
        $name = $entry.FullName -replace '\\', '/'
        $name = $name.TrimStart('./')
        if ($name -ne "navi.exe" -or $name.Contains("..") -or $name.Contains("/")) {
            Write-Err "Unsafe or unexpected zip member: $($entry.FullName)"
            Write-Err "Expected a single root file named navi.exe."
            exit 1
        }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $ExtractDir "navi.exe"), $true)
    } finally {
        $zip.Dispose()
    }

    $BinaryPath = Join-Path $ExtractDir "navi.exe"
    if (-not (Test-Path $BinaryPath)) {
        Write-Err "Extraction failed: navi.exe missing."
        exit 1
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $Dest = Join-Path $InstallDir "navi.exe"
    $TmpDest = "$Dest.tmp"
    Copy-Item -Path $BinaryPath -Destination $TmpDest -Force
    Move-Item -Path $TmpDest -Destination $Dest -Force
    Write-Ok "NAVI v${Version} installed to $Dest"

    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($CurrentPath -notlike "*$InstallDir*") {
        Write-Warn ""
        Write-Warn "$InstallDir is not in your PATH."
        Write-Warn ""

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
