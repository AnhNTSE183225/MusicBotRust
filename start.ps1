# ==============================================================================
# MusicBot Backend Production Launcher (PowerShell / Windows)
# ==============================================================================
# Runs pre-compiled MusicBot Rust binary in background with PID tracking (bot.pid)
# ==============================================================================

param(
    [switch]$Foreground
)

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$logsDir = Join-Path $root "logs"
if (-not (Test-Path $logsDir)) {
    New-Item -ItemType Directory -Path $logsDir -Force | Out-Null
}

$startLogFile = Join-Path $logsDir "start.log"
$pidFile = Join-Path $root "bot.pid"
$runnerLog = Join-Path $logsDir "bot_runner.log"
$runnerErr = Join-Path $logsDir "bot_runner_err.log"

function Write-RunnerLog {
    param([string]$Message, [string]$ForegroundColor = "White")
    $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $fileMsg = if ($Message) { "[$ts] $Message" } else { "" }
    Write-Host $Message -ForegroundColor $ForegroundColor
    Add-Content -Path $startLogFile -Value $fileMsg -Encoding utf8
}

Write-RunnerLog "==================================================" Cyan
Write-RunnerLog "    MusicBot Backend Background Runner (Windows)  " Cyan
Write-RunnerLog "==================================================" Cyan
Write-RunnerLog ""

# 1. Stop existing process via bot.pid if present
if (Test-Path $pidFile) {
    $oldPid = (Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
    if ($oldPid -and $oldPid.Trim() -match '^\d+$') {
        $pidNum = [int]$oldPid.Trim()
        $oldProc = Get-Process -Id $pidNum -ErrorAction SilentlyContinue
        if ($oldProc) {
            Write-RunnerLog "Stopping existing MusicBot process (PID $pidNum)..." Yellow
            Stop-Process -Id $pidNum -Force -ErrorAction SilentlyContinue
            & taskkill.exe /PID $pidNum /T /F 2>$null | Out-Null
            Write-RunnerLog "OK: Previous process stopped." Green
        }
    }
    Remove-Item -Path $pidFile -Force -ErrorAction SilentlyContinue
}

# 2. Terminate lingering port 8000 listeners
$port8000Conns = Get-NetTCPConnection -LocalPort 8000 -State Listen -ErrorAction SilentlyContinue
foreach ($conn in $port8000Conns) {
    if ($conn.OwningProcess -ne $PID -and $conn.OwningProcess -gt 0) {
        Write-RunnerLog "Freeing port 8000 from lingering process (PID $($conn.OwningProcess))..." Yellow
        Stop-Process -Id $conn.OwningProcess -Force -ErrorAction SilentlyContinue
        & taskkill.exe /PID $conn.OwningProcess /T /F 2>$null | Out-Null
    }
}

# 3. Artifact verification (build-start separation)
$releaseExe = Join-Path $root "target\release\musicbot.exe"
if (-not (Test-Path $releaseExe)) {
    Write-RunnerLog "Release binary missing. Automatically building with build.ps1..." Yellow
    & powershell.exe -ExecutionPolicy Bypass -File (Join-Path $root "build.ps1")
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $releaseExe)) {
        Write-RunnerLog "ERROR: Build failed. Cannot launch backend." Red
        exit 1
    }
}

# 4. Load environment from .env
$envFile = Join-Path $root ".env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        $line = $_.Trim()
        if ($line -and -not $line.StartsWith("#") -and $line -match "^([^=]+)=(.*)$") {
            $key = $matches[1].Trim()
            $val = $matches[2].Trim()
            if (($val.StartsWith('"') -and $val.EndsWith('"')) -or ($val.StartsWith("'") -and $val.EndsWith("'"))) {
                $val = $val.Substring(1, $val.Length - 2)
            }
            [System.Environment]::SetEnvironmentVariable($key, $val, "Process")
        }
    }
}

# Enforce DEV profile credentials if DEV is selected
$discordProfile = if ($env:DISCORD_PROFILE) { ($env:DISCORD_PROFILE -split '#')[0].Trim().ToUpper() } else { "DEV" }
if ($discordProfile -eq "DEV") {
    if ($env:DEV_DISCORD_TOKEN) { $env:DISCORD_TOKEN = $env:DEV_DISCORD_TOKEN }
    if ($env:DEV_DISCORD_CLIENT_ID) { $env:DISCORD_CLIENT_ID = $env:DEV_DISCORD_CLIENT_ID }
    if ($env:DEV_DISCORD_CLIENT_SECRET) { $env:DISCORD_CLIENT_SECRET = $env:DEV_DISCORD_CLIENT_SECRET }
    if (-not $env:RUST_LOG) { $env:RUST_LOG = "musicbot=debug,tower_http=debug,axum=debug,songbird=info" }
    Write-RunnerLog "[DEV] Active Discord Profile: DEV (Debug logs enabled)" Magenta
} else {
    if (-not $env:RUST_LOG) { $env:RUST_LOG = "musicbot=info,tower_http=info,songbird=info" }
}

if ($Foreground) {
    Set-Content -Path $pidFile -Value $PID
    Write-RunnerLog "Starting MusicBot in foreground (PID $PID) on port 8000..." Green
    & $releaseExe
    exit $LASTEXITCODE
}

# 5. Launch backend in background
Write-RunnerLog "Starting MusicBot binary in background..." Cyan

$runnerIn  = Join-Path $logsDir "bot_runner_in.empty"
if (-not (Test-Path $runnerIn)) {
    New-Item -ItemType File -Path $runnerIn -Force | Out-Null
}

$proc = Start-Process `
    -FilePath $releaseExe `
    -WorkingDirectory $root `
    -RedirectStandardInput $runnerIn `
    -RedirectStandardOutput $runnerLog `
    -RedirectStandardError $runnerErr `
    -PassThru `
    -WindowStyle Hidden

if (-not $proc -or $proc.HasExited) {
    Write-RunnerLog "ERROR: Backend failed to start immediately!" Red
    if (Test-Path $runnerErr) {
        Get-Content $runnerErr | Select-Object -Last 20 | ForEach-Object { Write-RunnerLog $_ Red }
    }
    exit 1
}

$botPid = $proc.Id
Set-Content -Path $pidFile -Value $botPid

Start-Sleep -Milliseconds 1200

$proc = Get-Process -Id $botPid -ErrorAction SilentlyContinue
if (-not $proc -or $proc.HasExited) {
    Write-RunnerLog "ERROR: Process exited during startup! Last logs:" Red
    if (Test-Path $runnerErr) {
        Get-Content $runnerErr | Select-Object -Last 25 | ForEach-Object { Write-RunnerLog $_ Red }
    } elseif (Test-Path $runnerLog) {
        Get-Content $runnerLog | Select-Object -Last 25 | ForEach-Object { Write-RunnerLog $_ Red }
    }
    Remove-Item -Path $pidFile -Force -ErrorAction SilentlyContinue
    exit 1
}

Write-RunnerLog ""
Write-RunnerLog "OK: MusicBot started in background (PID $botPid) on port 8000!" Green
Write-RunnerLog "Log: $runnerLog" Cyan
Write-RunnerLog "Err: $runnerErr" DarkGray
Write-RunnerLog ""
exit 0
