#!/usr/bin/env bash
# ==============================================================================
# MusicBot Backend Stopper (Linux / bash)
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$ROOT_DIR/bot.pid"

echo "Stopping MusicBot background services..."

if [ -f "$PID_FILE" ]; then
    PID="$(cat "$PID_FILE" 2>/dev/null || true)"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        echo "Stopping PID $PID..."
        kill -15 "$PID" 2>/dev/null || true
        sleep 1
        if kill -0 "$PID" 2>/dev/null; then
            kill -9 "$PID" 2>/dev/null || true
        fi
    fi
    rm -f "$PID_FILE"
fi

# Fallback: kill lingering musicbot binaries
pkill -f "$ROOT_DIR/target/release/musicbot" 2>/dev/null || true

echo "OK: MusicBot services stopped."
