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

# 2. Stop Tailscale Funnel if active
TS_PID_FILE="$ROOT_DIR/tailscale_funnel.pid"
sudo -n -v 2>/dev/null || true
if [ -f "$TS_PID_FILE" ]; then
    TS_PID=$(cat "$TS_PID_FILE" 2>/dev/null || true)
    if [ -n "$TS_PID" ]; then
        echo "Stopping Tailscale Funnel (PID $TS_PID)..."
        kill -9 "$TS_PID" 2>/dev/null || sudo -n kill -9 "$TS_PID" 2>/dev/null || true
        echo "Tailscale Funnel stopped."
    fi
    rm -f "$TS_PID_FILE"
fi

# Fallback: check for any lingering tailscale funnel processes
REMAINING_TS=$(ps -ef | grep '[t]ailscale funnel.*8000' | awk '{print $2}' || true)
if [ -n "$REMAINING_TS" ]; then
    for pid in $REMAINING_TS; do
        if [ "$pid" != "$$" ]; then
            kill -9 "$pid" 2>/dev/null || sudo -n kill -9 "$pid" 2>/dev/null || true
        fi
    done
fi

if command -v tailscale >/dev/null 2>&1; then
    (sudo -n tailscale funnel reset 2>/dev/null || tailscale funnel reset 2>/dev/null) || true
    (sudo -n tailscale serve reset 2>/dev/null || tailscale serve reset 2>/dev/null) || true
fi

# 3. Release port 8000 if bound
if command -v fuser >/dev/null 2>&1; then
    fuser -k 8000/tcp >/dev/null 2>&1 || true
fi

echo "OK: MusicBot services stopped."
