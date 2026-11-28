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

# Phase 0: Zero-Assumption Toolchain Check
try_pkg_install() {
    local apt_pkgs="$1" dnf_pkgs="$2" pacman_pkgs="$3"
    command -v sudo &>/dev/null || return 1
    if command -v apt-get &>/dev/null; then
        sudo apt-get update -y && sudo apt-get install -y $apt_pkgs && return 0
    elif command -v dnf &>/dev/null; then
        sudo dnf install -y $dnf_pkgs && return 0
    elif command -v pacman &>/dev/null; then
        sudo pacman -Sy --noconfirm $pacman_pkgs && return 0
    fi
    return 1
}

if ! command -v cc &>/dev/null || ! command -v pkg-config &>/dev/null; then
    try_pkg_install "build-essential pkg-config libssl-dev curl" "gcc gcc-c++ make pkgconfig openssl-devel curl" "base-devel pkgconf openssl curl" || true
    if ! command -v cc &>/dev/null || ! command -v pkg-config &>/dev/null; then
        echo "ERROR: Missing C compiler/linker or pkg-config. Install: sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev" >&2
        exit 1
    fi
fi

if ! command -v cargo &>/dev/null || ! command -v rustc &>/dev/null; then
    if command -v curl &>/dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y || true
        [ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env" || true
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! command -v cargo &>/dev/null || ! command -v rustc &>/dev/null; then
        echo "ERROR: Missing Rust toolchain (https://rustup.rs/). Run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y" >&2
        exit 1
    fi
fi

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
