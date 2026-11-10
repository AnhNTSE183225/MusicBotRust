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

# Ensure Rust/Cargo toolchain is loaded (required for non-interactive SSH sessions)
for cargo_env in "$HOME/.cargo/env" "/root/.cargo/env"; do
    if [ -f "$cargo_env" ]; then
        # shellcheck disable=SC1091
        source "$cargo_env"
        break
    fi
done

for cargo_bin in "$HOME/.cargo/bin" "/usr/local/cargo/bin" "/snap/bin"; do
    if [ -d "$cargo_bin" ]; then
        export PATH="$cargo_bin:$PATH"
    fi
done

# If cargo is still not found, automatically bootstrap Rust toolchain via rustup
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust toolchain not found on server. Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: Failed to locate or install cargo! Please ensure Rust is available on this system."
    exit 1
fi

echo "Using: $(cargo --version)"

if [ "${1:-}" = "-Clean" ] || [ "${1:-}" = "--clean" ]; then
    echo "Cleaning build artifacts..."
    cargo clean
fi

echo "Compiling Rust binary in release mode (--release)..."
cargo build --release

RELEASE_BIN="$ROOT_DIR/target/release/musicbot"
if [ ! -f "$RELEASE_BIN" ]; then
    echo "ERROR: Target binary not found at $RELEASE_BIN"
    exit 1
fi

BIN_SIZE="$(du -h "$RELEASE_BIN" | cut -f1)"
echo "Build complete! Binary size: $BIN_SIZE"
echo "Launch with ./start.sh"
