# VectorCode installer for Windows
# Usage: irm https://raw.githubusercontent.com/alejandro-technology/vectorcode/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "alejandro-technology/vectorcode"
$BinaryName = "vectorcode"
$InstallDir = Join-Path $env:LOCALAPPDATA "vectorcode\bin"

function Write-Info($msg) {
    Write-Host "✓ " -ForegroundColor Green -NoNewline
    Write-Host $msg
}

function Write-Warn($msg) {
    Write-Host "⚠ " -ForegroundColor Yellow -NoNewline
    Write-Host $msg
}

function Write-Err($msg) {
    Write-Host "✗ " -ForegroundColor Red -NoNewline
    Write-Host $msg
    exit 1
}

function Get-Arch {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64"   { return "x86_64" }
        "Arm64" { return "arm64" }
        default { Write-Err "Unsupported architecture: $arch" }
    }
}

function Install-Binary {
    $arch = Get-Arch
    $zipName = "${BinaryName}-windows-${arch}.zip"
    $url = "https://github.com/${Repo}/releases/latest/download/${zipName}"

    Write-Info "Downloading VectorCode for Windows/$arch..."

    $tempDir = Join-Path $env:TEMP "vectorcode-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    try {
        $zipPath = Join-Path $tempDir $zipName
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    } catch {
        Write-Err "Failed to download: $_. Check https://github.com/${Repo}/releases for available releases."
    }

    # Create install directory
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    # Extract
    Write-Info "Extracting to $InstallDir..."
    Expand-Archive -Path $zipPath -DestinationPath $InstallDir -Force

    # Verify binary exists
    $exePath = Join-Path $InstallDir "${BinaryName}.exe"
    if (-not (Test-Path $exePath)) {
        Write-Err "Binary not found after extraction at $exePath"
    }

    # Add to PATH if not already present
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
        Write-Info "Added $InstallDir to user PATH"
    } else {
        Write-Info "$InstallDir already in PATH"
    }

    # Cleanup
    Remove-Item -Path $tempDir -Recurse -Force

    Write-Host ""
    Write-Info "VectorCode installed successfully!"
    Write-Info "Location: $exePath"
    Write-Host ""
    Write-Host "  Restart your terminal, then run:" -ForegroundColor Cyan
    Write-Host "    vectorcode init" -ForegroundColor White
    Write-Host ""
}

Install-Binary
