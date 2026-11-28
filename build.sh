#!/usr/bin/env bash
# ==============================================================================
# MusicBot Backend Production Build Script (Linux / bash)
# ==============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$ROOT_DIR/logs"

echo "=================================================="
echo "    MusicBot Rust Backend Production Build (Linux) "
echo "=================================================="

# Stop running process if running
if [ -f "$ROOT_DIR/bot.pid" ]; then
    PID="$(cat "$ROOT_DIR/bot.pid" 2>/dev/null || true)"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        echo "Stopping running MusicBot (PID $PID)..."
        kill -15 "$PID" 2>/dev/null || true
        sleep 0.5
    fi
fi

# Load Cargo environment for non-interactive shells
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env" || true
export PATH="$HOME/.cargo/bin:$PATH"

cd "$ROOT_DIR"

if [ -z "${CARGO_TARGET_DIR:-}" ] && ! cargo config get build.target-dir >/dev/null 2>&1; then
    export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/cargo-target"
fi
TARGET_ROOT="${CARGO_TARGET_DIR:-$(cargo config get build.target-dir 2>/dev/null | tr -d '"' || echo "$ROOT_DIR/target")}"

if [ "${1:-}" = "-Clean" ] || [ "${1:-}" = "--clean" ]; then
    echo "Cleaning build artifacts for musicbot..."
    rm -rf "$ROOT_DIR/target"
    cargo clean -p musicbot || true
fi

echo "Compiling Rust binary in release mode (--release)..."
cargo build --release

mkdir -p "$ROOT_DIR/target/release"
BUILT_BIN="$TARGET_ROOT/release/musicbot"
RELEASE_BIN="$ROOT_DIR/target/release/musicbot"
if [ ! -f "$BUILT_BIN" ]; then
    echo "ERROR: Target binary not found at $BUILT_BIN"
    exit 1
fi
if [ "$(realpath "$BUILT_BIN")" != "$(realpath -m "$RELEASE_BIN")" ]; then
    cp -f "$BUILT_BIN" "$RELEASE_BIN"
fi

BIN_SIZE="$(du -h "$RELEASE_BIN" | cut -f1)"
echo "Build complete! Binary size: $BIN_SIZE"
echo "Launch with ./start.sh"
