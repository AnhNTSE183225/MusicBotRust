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

# 2. Clean if requested & 3. Compile in optimized release mode (Shared Cargo Target Cache)
Push-Location $root
try {
    $meta = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $localTargetDir = Join-Path $root "target"
    $cargoTargetDir = $meta.target_directory
    if ([System.IO.Path]::GetFullPath($cargoTargetDir) -ieq [System.IO.Path]::GetFullPath($localTargetDir)) {
        $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA "cargo-target"
        $cargoTargetDir = $env:CARGO_TARGET_DIR
    }

    $pkg = $meta.packages | Select-Object -First 1
    $pkgName = $pkg.name

    if ($Clean) {
        Write-Host "[1/2] Cleaning previous build artifacts for '$pkgName'..." -ForegroundColor Yellow
        if (Test-Path $localTargetDir) { Remove-Item -Recurse -Force $localTargetDir -ErrorAction SilentlyContinue }
        if ($pkgName) { & cargo clean -p $pkgName } else { & cargo clean }
    }

    Write-Host "[2/2] Compiling Rust backend with maximum optimizations (--release, shared target: $cargoTargetDir)..." -ForegroundColor Cyan
    & cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed with exit code $LASTEXITCODE."
    }

    $builtExe = Join-Path $cargoTargetDir "release\musicbot.exe"
    if (-not (Test-Path $builtExe)) {
        throw "Build completed but target binary was not found at $builtExe"
    }
    $localReleaseDir = Join-Path $localTargetDir "release"
    if (-not (Test-Path $localReleaseDir)) {
        New-Item -ItemType Directory -Path $localReleaseDir -Force | Out-Null
    }
    $releaseExe = Join-Path $localReleaseDir "musicbot.exe"
    if ([System.IO.Path]::GetFullPath($builtExe) -ine [System.IO.Path]::GetFullPath($releaseExe)) {
        Copy-Item -Path $builtExe -Destination $releaseExe -Force
    }
} finally {
    Pop-Location
}

# 4. Validate output artifact
if (-not (Test-Path $releaseExe)) {
    throw "Build completed but staged binary was not found at $releaseExe"
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
