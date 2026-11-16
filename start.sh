#!/usr/bin/env bash
# ==============================================================================
# MusicBot Backend Production Launcher (Linux / bash)
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$ROOT_DIR/logs"

PID_FILE="$ROOT_DIR/bot.pid"
RUNNER_LOG="$ROOT_DIR/logs/bot_runner.log"
RELEASE_BIN="$ROOT_DIR/target/release/musicbot"

echo "=================================================="
echo "    MusicBot Background Runner (Linux)            "
echo "=================================================="

# 1. Verify decoupled host logging systemd services are active
MISSING_LOGGING_SERVICES=()
if ! command -v systemctl >/dev/null 2>&1; then
    echo "ERROR: 'systemctl' not found. Linux production observability requires systemd." >&2
    echo "Install and enable the centralized TECHSTACK logging stack (observability/install.sh) first." >&2
    exit 1
fi

for svc in victoria-logs.service systemd-journal-upload.service; do
    if ! systemctl is-active --quiet "$svc"; then
        MISSING_LOGGING_SERVICES+=("$svc")
    fi
done

if [ "${#MISSING_LOGGING_SERVICES[@]}" -gt 0 ]; then
    echo "==================================================" >&2
    echo "ERROR: Required logging systemd service(s) are NOT running:" >&2
    for svc in "${MISSING_LOGGING_SERVICES[@]}"; do
        echo "  - $svc" >&2
    done
    echo "" >&2
    echo "Turn on the systemd logging services with:" >&2
    echo "  sudo systemctl enable --now victoria-logs.service systemd-journal-upload.service" >&2
    echo "" >&2
    echo "If not installed yet on this host, run the centralized TECHSTACK installer:" >&2
    echo "  sudo bash <TECHSTACK_REPO>/observability/install.sh" >&2
    echo "==================================================" >&2
    exit 1
fi
echo "OK: Host observability active (victoria-logs.service + systemd-journal-upload.service)."

# 2. Stop existing process if running
if [ -f "$PID_FILE" ]; then
    OLD_PID="$(cat "$PID_FILE" 2>/dev/null || true)"
    if [ -n "$OLD_PID" ] && kill -0 "$OLD_PID" 2>/dev/null; then
        echo "Stopping existing MusicBot process (PID $OLD_PID)..."
        kill -15 "$OLD_PID" 2>/dev/null || true
        sleep 1
        if kill -0 "$OLD_PID" 2>/dev/null; then
            kill -9 "$OLD_PID" 2>/dev/null || true
        fi
    fi
    rm -f "$PID_FILE"
fi

# 2. Check build artifacts
if [ ! -f "$RELEASE_BIN" ]; then
    echo "Release binary missing. Automatically building with ./build.sh..."
    bash "$ROOT_DIR/build.sh"
fi

# 3. Ensure yt-dlp and runtime tools are available
export PATH="$ROOT_DIR/bin:$HOME/.local/bin:/usr/local/bin:$PATH"
if ! command -v yt-dlp >/dev/null 2>&1; then
    mkdir -p "$ROOT_DIR/bin"
    if [ ! -f "$ROOT_DIR/bin/yt-dlp" ]; then
        echo "yt-dlp not found in PATH. Downloading standalone yt-dlp binary..."
        curl -sL "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp" -o "$ROOT_DIR/bin/yt-dlp"
        chmod +x "$ROOT_DIR/bin/yt-dlp"
    fi
    export PATH="$ROOT_DIR/bin:$PATH"
fi

# 4. Load .env
if [ -f "$ROOT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$ROOT_DIR/.env"
    set +a
fi

if [ "${DISCORD_PROFILE:-DEV}" = "DEV" ]; then
    export DISCORD_TOKEN="${DEV_DISCORD_TOKEN:-$DISCORD_TOKEN}"
    export DISCORD_CLIENT_ID="${DEV_DISCORD_CLIENT_ID:-$DISCORD_CLIENT_ID}"
    export DISCORD_CLIENT_SECRET="${DEV_DISCORD_CLIENT_SECRET:-$DISCORD_CLIENT_SECRET}"
    export RUST_LOG="${RUST_LOG:-musicbot=debug,tower_http=debug,axum=debug,songbird=info}"
    echo "[DEV] Active Discord Profile: DEV (Debug logs enabled)"
else
    export RUST_LOG="${RUST_LOG:-musicbot=info,tower_http=info,songbird=info}"
fi
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

# 5. Start in background (stream all stdout/stderr—including panics & exiting logs—into journald -> VictoriaLogs)
echo "Starting MusicBot in background..."
if command -v systemd-cat >/dev/null 2>&1; then
    nohup "$RELEASE_BIN" < /dev/null > >(tee -a "$RUNNER_LOG" | systemd-cat -t musicbot) 2>&1 &
else
    nohup "$RELEASE_BIN" < /dev/null >> "$RUNNER_LOG" 2>&1 &
fi
BOT_PID=$!
echo "$BOT_PID" > "$PID_FILE"

sleep 1.2

if ! kill -0 "$BOT_PID" 2>/dev/null; then
    echo "ERROR: MusicBot failed to start! Last log lines:"
    tail -n 25 "$RUNNER_LOG"
    rm -f "$PID_FILE"
    exit 1
fi

echo "OK: MusicBot started in background (PID $BOT_PID) on port 8000!"
echo "Log: $RUNNER_LOG"

# 5. Starting Tailscale Funnel in Background
TS_PID_FILE="$ROOT_DIR/tailscale_funnel.pid"
TS_LOG_FILE="$ROOT_DIR/logs/tailscale_funnel.log"
TS_RUNNING=false
if [ -f "$TS_PID_FILE" ]; then
    TS_PID=$(cat "$TS_PID_FILE" 2>/dev/null || true)
    if [ -n "$TS_PID" ] && kill -0 "$TS_PID" 2>/dev/null; then
        TS_RUNNING=true
    fi
fi

if [ "$TS_RUNNING" = false ]; then
    ACTIVE_TS_PID=$(pgrep -f "tailscale funnel.*8000" | head -n 1 || true)
    if [ -n "$ACTIVE_TS_PID" ]; then
        echo "$ACTIVE_TS_PID" > "$TS_PID_FILE"
        TS_RUNNING=true
    fi
fi

if [ "$TS_RUNNING" = true ]; then
    echo "Tailscale Funnel is already running (PID $(cat "$TS_PID_FILE" 2>/dev/null))."
else
    if command -v tailscale >/dev/null 2>&1; then
        echo "Starting Tailscale Funnel on port 8000..."
        sudo -n -v 2>/dev/null || true
        (sudo -n tailscale funnel 8000 2>/dev/null || tailscale funnel 8000) > "$TS_LOG_FILE" 2>&1 &
        TS_SUB_PID=$!
        sleep 1
        ACTUAL_TS_PID=$(pgrep -f "tailscale funnel.*8000" | head -n 1 || true)
        echo "${ACTUAL_TS_PID:-$TS_SUB_PID}" > "$TS_PID_FILE"
        echo "Tailscale funnel started! Logs are in $TS_LOG_FILE"
    fi
fi

echo "All services started successfully in the background!"
