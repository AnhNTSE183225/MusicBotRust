# ==============================================================================
# MusicBot Backend Stopper (PowerShell / Windows)
# ==============================================================================
# Stops running MusicBot process cleanly via bot.pid and frees port 8000
# ==============================================================================

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$pidFile = Join-Path $root "bot.pid"

Write-Host "Stopping MusicBot Background Services..." -ForegroundColor Cyan

$stopped = $false

if (Test-Path $pidFile) {
    $pidContent = Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pidContent -and $pidContent.Trim() -match '^\d+$') {
        $pidNum = [int]$pidContent.Trim()
        $proc = Get-Process -Id $pidNum -ErrorAction SilentlyContinue
        if ($proc) {
            Write-Host "Stopping MusicBot process (PID $pidNum)..." -ForegroundColor Yellow
            Stop-Process -Id $pidNum -Force -ErrorAction SilentlyContinue
            & taskkill.exe /PID $pidNum /T /F 2>$null | Out-Null
            $stopped = $true
        }
    }
    Remove-Item -Path $pidFile -Force -ErrorAction SilentlyContinue
}

# Also ensure port 8000 listeners are cleared
$port8000Conns = Get-NetTCPConnection -LocalPort 8000 -State Listen -ErrorAction SilentlyContinue
foreach ($conn in $port8000Conns) {
    if ($conn.OwningProcess -gt 4) {
        Write-Host "Terminating lingering process on port 8000 (PID $($conn.OwningProcess))..." -ForegroundColor Yellow
        Stop-Process -Id $conn.OwningProcess -Force -ErrorAction SilentlyContinue
        & taskkill.exe /PID $conn.OwningProcess /T /F 2>$null | Out-Null
        $stopped = $true
    }
}

# Stop any running musicbot.exe processes
$lingering = Get-Process -Name "musicbot" -ErrorAction SilentlyContinue
foreach ($p in $lingering) {
    Write-Host "Terminating lingering musicbot process (PID $($p.Id))..." -ForegroundColor Yellow
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    & taskkill.exe /PID $p.Id /T /F 2>$null | Out-Null
    $stopped = $true
}

Write-Host ""
Write-Host "OK: MusicBot services stopped." -ForegroundColor Green
Write-Host ""
exit 0
