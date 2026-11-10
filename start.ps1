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

# Ensure port 8000 is completely released by the OS socket stack
$portWaitCount = 0
while ((Get-NetTCPConnection -LocalPort 8000 -State Listen -ErrorAction SilentlyContinue) -and ($portWaitCount -lt 15)) {
    Start-Sleep -Milliseconds 200
    $portWaitCount++
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

# 5. Tailscale Funnel Automation
if (Get-Command tailscale -ErrorAction SilentlyContinue) {
    Write-RunnerLog "Checking Tailscale Funnel status on port 8000..." Cyan
    $funnelStatus = (& tailscale funnel status 2>&1 | Out-String).Trim()
    if ($funnelStatus -match "(?:8000|https://)") {
        Write-RunnerLog "Tailscale Funnel is already active on port 8000." Yellow
    } else {
        Write-RunnerLog "Starting Tailscale Funnel on port 8000..." Cyan
        $funnelOut = (& tailscale funnel --bg --yes 8000 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -eq 0) {
            Write-RunnerLog "OK: Tailscale Funnel exposed on port 8000." Green
        } else {
            Write-RunnerLog "WARNING: Failed to start Tailscale Funnel: $funnelOut" Yellow
        }
    }
}

# 6. Launch backend in background (detached daemon with native file handles and NO window)
Write-RunnerLog "Starting MusicBot binary in background..." Cyan

# Spawn fully detached process via WMI with ShowWindow = 0 (SW_HIDE) so NO window or terminal pops up,
# and parent PowerShell exit never breaks I/O pipes or terminates the process.
$escapedExe = (Resolve-Path $releaseExe).Path
$runnerLogResolved = (Resolve-Path $runnerLog -ErrorAction SilentlyContinue)
if (-not $runnerLogResolved) {
    New-Item -ItemType File -Path $runnerLog -Force | Out-Null
    $runnerLogResolved = (Resolve-Path $runnerLog).Path
} else {
    $runnerLogResolved = $runnerLogResolved.Path
}

$cmdLine = "cmd.exe /c `"`"$escapedExe`" >> `"$runnerLogResolved`" 2>&1`""
$startup = New-CimInstance -ClassName Win32_ProcessStartup -Property @{ ShowWindow = [uint16]0 } -ClientOnly

$res = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
    CommandLine = $cmdLine
    CurrentDirectory = $root
    ProcessStartupInformation = $startup
}

if ($res.ReturnValue -ne 0) {
    Write-RunnerLog "ERROR: Failed to launch backend via Win32_Process (Return code: $($res.ReturnValue))" Red
    exit 1
}

# Allow startup, DB connection, and Discord handshake
Start-Sleep -Milliseconds 2500

# Locate the actual musicbot.exe process
$botProc = Get-Process -Name "musicbot" -ErrorAction SilentlyContinue | Select-Object -First 1

if (-not $botProc -or $botProc.HasExited) {
    Write-RunnerLog "ERROR: Process exited during startup! Last logs:" Red
    if (Test-Path $runnerLog) {
        Get-Content $runnerLog | Select-Object -Last 30 | ForEach-Object { Write-RunnerLog $_ Red }
    }
    Remove-Item -Path $pidFile -Force -ErrorAction SilentlyContinue
    exit 1
}

$botPid = $botProc.Id
Set-Content -Path $pidFile -Value $botPid

Write-RunnerLog ""
Write-RunnerLog "OK: MusicBot started in background (PID $botPid) on port 8000!" Green
Write-RunnerLog "Log: $runnerLog" Cyan
Write-RunnerLog ""
exit 0
