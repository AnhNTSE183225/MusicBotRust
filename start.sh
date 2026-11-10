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

# 1. Stop existing process if running
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

# 3. Load .env
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

# 4. Start in background
echo "Starting MusicBot in background..."
nohup "$RELEASE_BIN" >> "$RUNNER_LOG" 2>&1 &
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
