# ==============================================================================
# MusicBot Backend Production Build Script (PowerShell / Windows)
# ==============================================================================
# Builds the high-performance Rust backend in optimized release mode.
# Usage:
#   .\build.ps1           # Standard release build
#   .\build.ps1 -Clean    # Rebuild from scratch (cargo clean)
# ==============================================================================

param(
    [switch]$Clean
)

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$logsDir = Join-Path $root "logs"
if (-not (Test-Path $logsDir)) {
    New-Item -ItemType Directory -Path $logsDir -Force | Out-Null
}

$sw = [System.Diagnostics.Stopwatch]::StartNew()

Write-Host ""
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host "       MusicBot Rust Backend Production Build" -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host ""

# 1. Terminate running backend to unlock binary on Windows (prevents Error 5: Access is denied)
$targetExeName = "musicbot"
$runningProcs = Get-Process -Name $targetExeName -ErrorAction SilentlyContinue
foreach ($p in $runningProcs) {
    Write-Host "  Stopping running backend process (PID $($p.Id)) to unlock binary..." -ForegroundColor Yellow
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
}

# 2. Clean if requested
if ($Clean) {
    Write-Host "[1/2] Cleaning previous build artifacts..." -ForegroundColor Yellow
    Push-Location $root
    try {
        & cargo clean
    } finally {
        Pop-Location
    }
}

# 3. Compile in optimized release mode
Write-Host "[2/2] Compiling Rust backend with maximum optimizations (--release)..." -ForegroundColor Cyan
Push-Location $root
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

# 4. Validate output artifact
$releaseExe = Join-Path $root "target\release\musicbot.exe"
if (-not (Test-Path $releaseExe)) {
    throw "Build completed but target binary was not found at $releaseExe"
}

$fileSizeMb = (Get-Item $releaseExe).Length / 1MB
$sw.Stop()

Write-Host ""
Write-Host "==================================================" -ForegroundColor Green
Write-Host (" Backend build complete in {0:N1}s! Binary size: {1:N2} MB" -f $sw.Elapsed.TotalSeconds, $fileSizeMb) -ForegroundColor Green
Write-Host " Binary: $releaseExe" -ForegroundColor DarkGray
Write-Host " Ready for instantaneous launch with .\start.ps1" -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Green
Write-Host ""
exit 0
