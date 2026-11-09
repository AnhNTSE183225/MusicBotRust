# BACKEND_TECHSTACK.md: High-Performance Rust Architecture

A reference specification for building an ultra-low latency, low-footprint pure backend service in Rust designed for sub-millisecond p99 latency, minimal idle memory (10–30 MB), and seamless execution across **local Windows development** and **low-power Raspberry Pi production**.

---

## 1. Toolchain & Core Prerequisites

| Component | Role | Documentation & Reference |
| :--- | :--- | :--- |
| **Rust Toolchain (`rustup` / `cargo` / `rustc`)** | Core systems language toolchain (zero GC, deterministic borrow-checker lifetimes) | • [rustup Installer](https://rustup.rs/)<br>• [The Rust Programming Language Book](https://doc.rust-lang.org/book/)<br>• [Rust Standard Library Reference](https://doc.rust-lang.org/std/) |
| **LLVM Toolchain / MSVC** | Native binary code generation (x86_64 Windows & ARM Linux compilation) | • Windows: Visual Studio C++ Build Tools (MSVC ABI)<br>• Linux target: `aarch64-unknown-linux-musl` |
| **`cross` (`cross-rs`)** | Zero-setup cross-compilation from Windows to ARM Linux via isolated build containers | • [cross-rs GitHub](https://github.com/cross-rs/cross) |
| **`just` (Command Runner)** | Unified, cross-platform task runner replacing fragmented `.bat` and `.sh` scripts | • [just Manual](https://just.systems/man/en/) |
| **Cargo Subcommands** | Development automation & validation | • [cargo-watch](https://crates.io/crates/cargo-watch) (Auto-reload / watch changes)<br>• [sqlx-cli](https://crates.io/crates/sqlx-cli) (`cargo sqlx prepare` for offline query caching)<br>• [cargo-audit](https://crates.io/crates/cargo-audit) (Vulnerability scanner)<br>• [cargo-flamegraph](https://crates.io/crates/cargo-flamegraph) (CPU profiling & flamegraph generation) |

---

## 2. Environment Setup Guide (Windows & Linux)

### Windows 10 / 11 Setup

Run the following commands in PowerShell (as Administrator):

```powershell
# 1. Install Microsoft Visual Studio C++ Build Tools (MSVC Linker)
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --add Microsoft.VisualStudio.Workload.VCTools"

# 2. Install Rustup (Installs rustc compiler and cargo package manager)
winget install Rustlang.Rustup

# 3. Restart your PowerShell terminal, then verify the installation
rustc --version
cargo --version

# 4. Optional: Install productivity CLI utilities via Cargo
cargo install cargo-watch                                      # Hot-reloading daemon
cargo install sqlx-cli --no-default-features --features sqlite # Database migrations & query caching
cargo install just                                             # Cross-platform command shortcut runner

# 5. Optional: For cross-compiling to Linux ARM from Windows
winget install Docker.DockerDesktop
cargo install cross --git https://github.com/cross-rs/cross
```

### Linux Setup (Ubuntu / Debian / Raspberry Pi OS / Fedora / Arch)

Run the following commands in your Linux terminal:

```bash
# 1. Install system C compiler, linker, and development headers
# (Debian / Ubuntu / Raspberry Pi OS)
sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev curl

# (Fedora)
# sudo dnf groupinstall "Development Tools" && sudo dnf install -y openssl-devel curl

# (Arch Linux)
# sudo pacman -S base-devel openssl curl

# 2. Install Rustup via official installer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# 3. Source Cargo environment in current shell session
source "$HOME/.cargo/env"

# 4. Verify installation
rustc --version
cargo --version

# 5. Optional: Install productivity utilities
cargo install cargo-watch
cargo install sqlx-cli --no-default-features --features sqlite
cargo install just
```

---

## 3. Server Framework & Runtime Engine

```text
┌────────────────────────────────────────────────────────┐
│  Axum: Routing, Extractors, Typed REST API Handlers    │
├────────────────────────────────────────────────────────┤
│  Tower & Tower-HTTP: CORS, Compression, Tracing        │
├────────────────────────────────────────────────────────┤
│  Tokio: Multi-threaded Work-Stealing Async Runtime     │
├────────────────────────────────────────────────────────┤
│  OS Socket Layer: Windows IOCP  /  Linux epoll         │
└────────────────────────────────────────────────────────┘
```

### Tokio (Async Executor & Event Loop)
* **Role:** Multi-threaded work-stealing task scheduler handling asynchronous network I/O, timers, and cooperative green-thread multitasking.
* **Performance Profile:** Microsecond task-switching with zero OS thread thrashing. Uses Windows I/O Completion Ports (IOCP) natively on Windows and `epoll` on Linux.
* **Documentation:**
  * Official Guide: [tokio.rs](https://tokio.rs/)
  * API Reference: [docs.rs/tokio](https://docs.rs/tokio)
  * GitHub: [tokio-rs/tokio](https://github.com/tokio-rs/tokio)

### Axum (HTTP Web Framework)
* **Role:** Ergonomic, macro-light routing engine backed directly by the Tokio core team.
* **Performance Profile:** Direct compile-time type resolution for handlers and extractors (zero runtime string reflection, no regex routing overhead).
* **Documentation:**
  * API Reference: [docs.rs/axum](https://docs.rs/axum)
  * GitHub: [tokio-rs/axum](https://github.com/tokio-rs/axum)

### Tower & Tower-HTTP (Service & Middleware Layer)
* **Role:** Standardized `Service` trait abstraction providing composable, zero-cost HTTP middleware.
* **Modules:** `CorsLayer` (CORS negotiation), `CompressionLayer` (Brotli/Gzip stream compression), `TraceLayer` (distributed spans), `TimeoutLayer` (per-request latency bounds).
* **Documentation:**
  * API Reference: [docs.rs/tower](https://docs.rs/tower) | [docs.rs/tower-http](https://docs.rs/tower-http)
  * GitHub: [tower-rs/tower](https://github.com/tower-rs/tower) | [tower-rs/tower-http](https://github.com/tower-rs/tower-http)

---

## 4. Data Persistence, Drivers & Serialization

### SQLx (Async SQL Driver & Connection Pool)
* **Role:** Pure-Rust asynchronous database driver supporting SQLite and PostgreSQL.
* **Database Strategy:**
  * **Primary (Raspberry Pi & Rapid Projects): SQLite with WAL Mode (`mode=rwc`, `journal_mode=WAL`).** Runs entirely in-process with **0 MB idle daemon memory overhead**, avoiding the 80–150 MB RAM cost of running an external database server on constrained devices.
  * **Secondary (Distributed/Multi-Node): PostgreSQL.** Can be switched via cargo feature flag if multi-instance write concurrency is required.
* **Compile-Time Safety & Cross-Compilation (`cargo sqlx prepare`):**
  * Validates SQL queries, schema, and types at compile time.
  * For cross-compiling on Windows targeting ARM Linux without connecting to a live database, use offline mode (`cargo sqlx prepare`), which generates `sqlx-data.json`.
* **Documentation:**
  * API Reference: [docs.rs/sqlx](https://docs.rs/sqlx)
  * GitHub: [transact-rs/sqlx](https://github.com/transact-rs/sqlx)

### Serde & Serde JSON (Serialization Engine)
* **Role:** Compile-time trait derive system for encoding and decoding Rust data structures to/from JSON.
* **Performance Profile:** Zero-copy deserialization (`serde::Deserialize`) that can borrow directly from raw byte buffers (`&str`, `&[u8]`) without allocating intermediate heap objects.
* **Documentation:**
  * Official Manual: [serde.rs](https://serde.rs/)
  * API Reference: [docs.rs/serde](https://docs.rs/serde) | [docs.rs/serde_json](https://docs.rs/serde_json)
  * GitHub: [serde-rs/serde](https://github.com/serde-rs/serde)

### Validator (Data Validation)
* **Role:** Declarative struct validation via derive macros.
* **Performance Profile:** Executes range checks, string lengths, regex matches, and format rules without allocations, executed immediately inside Axum extractors.
* **Documentation:**
  * API Reference: [docs.rs/validator](https://docs.rs/validator)
  * GitHub: [Keats/validator](https://github.com/Keats/validator)

---

## 5. Systems, Memory, Configuration & Telemetry Layer

### Hierarchical YAML Configuration (`config` Crate)
* **Role:** Unified, single-location configuration using YAML with automatic inheritance and cascading overrides.
* **Design Pattern (Base + Production Delta):**
  * `config/default.yaml`: Complete baseline configuration containing all keys and defaults (optimized for local Windows development).
  * `config/prod.yaml`: **Minimal override file** containing *only the fields that change* for production (e.g. `server.host: "0.0.0.0"`, `server.port: 80`, production database path). Any key omitted in `prod.yaml` automatically inherits the baseline value from `default.yaml`.
  * Optional environment variable overrides (`APP__*`).
* **Documentation:**
  * API Reference: [docs.rs/config](https://docs.rs/config)

### Mimalloc (Global Memory Allocator)
* **Role:** Replaces default memory allocators with Microsoft’s cache-conscious, compact allocator.
* **Performance Profile:** Eliminates lock contention in high-concurrency multi-threaded allocations; drops memory fragmentation by up to 30%.
* **Documentation:**
  * API Reference: [docs.rs/mimalloc](https://docs.rs/mimalloc)
  * GitHub: [microsoft/mimalloc](https://github.com/microsoft/mimalloc)

### Tracing & Tracing-Subscriber (Structured Telemetry)
* **Role:** Non-blocking, structured diagnostics built explicitly for asynchronous execution flows.
* **Key Advantage:** Unlike traditional line loggers, `tracing` attaches contextual spans that follow asynchronous tasks across threads and `.await` yield points.
* **Documentation:**
  * API Reference: [docs.rs/tracing](https://docs.rs/tracing) | [docs.rs/tracing-subscriber](https://docs.rs/tracing-subscriber)
  * GitHub: [tokio-rs/tracing](https://github.com/tokio-rs/tracing)

### Thiserror & Anyhow (Error Architecture)
* **Role:**
  * `thiserror`: Generates lightweight, zero-cost domain error enums for library/internal backend code without dynamic heap allocations.
  * `anyhow`: Ergonomic context wrapper for edge/main application initialization errors.
* **Documentation:**
  * API Reference: [docs.rs/thiserror](https://docs.rs/thiserror) | [docs.rs/anyhow](https://docs.rs/anyhow)
  * GitHub: [dtolnay/thiserror](https://github.com/dtolnay/thiserror)

### Argon2 & JSON Web Tokens (Security & Cryptography)
* **Role:** Password hashing and stateless authentication.
* **Crates:** `argon2` (memory-hard, side-channel resistant hashing), `jsonwebtoken` (compact, stateless signature validation).
* **Documentation:**
  * [docs.rs/argon2](https://docs.rs/argon2) | [docs.rs/jsonwebtoken](https://docs.rs/jsonwebtoken)

---

## 6. Complete Backend `Cargo.toml` Reference

```toml
[package]
name = "ultra-engine"
version = "0.1.0"
edition = "2024"

[dependencies]
# 1. Asynchronous Core & HTTP Layer
tokio = { version = "1", features = ["full"] }
axum = { version = "0.8", features = ["macros"] }
tower = { version = "0.5" }
tower-http = { version = "0.6", features = ["cors", "trace", "compression-full", "timeout"] }

# 2. Database & Connection Pooling (SQLite WAL mode default; switchable to postgres)
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "sqlite", "uuid", "chrono"] }

# 3. Serialization, Configuration & Validation
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
config = { version = "0.15", features = ["yaml"] }
validator = { version = "0.19", features = ["derive"] }

# 4. Telemetry, Tracing & Diagnostics
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
thiserror = "2.0"
anyhow = "1.0"

# 5. Security & Cryptography
argon2 = "0.5"
jsonwebtoken = "9.3"

# 6. Low-overhead Allocator
mimalloc = { version = "0.1", default-features = false }

[profile.release]
opt-level = 3        # Maximum optimizations
lto = "fat"          # Link-Time Optimization across all crates
codegen-units = 1    # Sacrifices compile speed for better optimization passes
panic = "abort"      # Strips stack-unwind tables; produces smaller, faster binaries
strip = true         # Automatically strips symbols from the output binary
```

> [!IMPORTANT]
> **CRITICAL PERFORMANCE RULE: Always Run & Build in Release Mode (`--release`)**
> * **Debug Mode (`cargo run` / `cargo build`):** Defaults to `opt-level = 0` with zero inlining, no SIMD auto-vectorization, and disabled loop unrolling. While compilation is fast, compute-intensive workloads—such as image decoding/resizing (`image` crate), audio/video metadata parsing, and cryptographic hashing—run **20x to 100x slower** (e.g., downscaling a photo takes 1,500–2,000 ms in debug vs. 15–25 ms in release).
> * **Release Mode (`cargo run --release` / `cargo build --release`):** Activates maximum LLVM optimizations (`opt-level = 3`), LTO, and CPU vector extensions. Always run with `--release` in development scripts and production to prevent artificial bottlenecks.

---

## 7. Architecture & Directory Blueprint

```text
my-project/
├── Cargo.toml
├── Justfile                   # Cross-platform task automation (dev, build, deploy)
├── sqlx-data.json             # Cached queries for offline cross-compilation
│
├── config/                    # [Configurations] All YAML files in ONE place
│   ├── default.yaml           # Complete baseline config & local defaults
│   └── prod.yaml              # Minimal overrides for Raspberry Pi production
│
├── migrations/                # [Database] SQL schema files
│   └── 20260101000000_init.sql
│
├── deploy/                    # [Build & Deploy Pipelines] Ops & infrastructure
│   ├── systemd/
│   │   └── myapp.service      # Linux systemd background service descriptor
│   └── scripts/
│       ├── build-linux.ps1    # Cross-compilation script targeting Linux
│       └── deploy-linux.ps1   # SCP binary/configs to remote Linux host and restart service
│
└── src/                       # [Pure Code] Application logic only
    ├── main.rs                # Application entrypoint, routing & graceful shutdown
    ├── config.rs              # Strongly typed YAML cascading loader
    ├── error.rs               # Central AppError enum + Axum IntoResponse
    ├── state.rs               # Shared AppState (SQLite pool, config)
    ├── routes/                # Route sub-routers
    │   ├── mod.rs
    │   ├── auth.rs
    │   └── items.rs
    ├── handlers/              # Endpoint business logic
    │   ├── auth_handler.rs
    │   └── item_handler.rs
    └── models/                # Domain entities & Serde DTOs
        ├── user.rs
        └── item.rs
```

---

## 8. Configuration Implementation Blueprint

### `config/default.yaml` (Complete Baseline / Local Defaults)
```yaml
server:
  host: "127.0.0.1"
  port: 8080

database:
  url: "sqlite://local.db?mode=rwc"
  max_connections: 5

auth:
  jwt_secret: "dev-insecure-secret-key-32-bytes"
  token_ttl_seconds: 86400
```

### `config/prod.yaml` (Minimal Production Delta)
```yaml
# Only specify values that differ from default.yaml
server:
  host: "0.0.0.0"
  port: 80

database:
  url: "sqlite:///var/lib/myapp/data.db?mode=rwc"
  max_connections: 10
```

### Cascading Config Loader (`src/config.rs`)
```rust
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub token_ttl_seconds: u64,
}

impl AppConfig {
    pub fn load(override_path: Option<&str>) -> Result<Self, config::ConfigError> {
        let mut builder = config::Config::builder()
            // 1. Always load baseline complete defaults
            .add_source(config::File::with_name("config/default.yaml").required(true));

        // 2. Cascade minimal override if specified (e.g. prod.yaml)
        if let Some(path) = override_path {
            builder = builder.add_source(config::File::with_name(path).required(false));
        }

        // 3. Allow optional ENV variable overrides (e.g. APP__AUTH__JWT_SECRET)
        builder = builder.add_source(config::Environment::with_prefix("APP").separator("__"));

        builder.build()?.try_deserialize()
    }
}
```

---

## 9. Development & Deployment Lifecycle

### Why Bare-Metal Static Binary Beats Docker on Raspberry Pi
* **Zero Container Daemon Bloat:** Running `dockerd` and `containerd` consumes **80–120 MB RAM** on a Pi before your application even boots. A native Rust binary runs in **10–25 MB RAM**.
* **Protects SD Cards:** Eliminates continuous Docker logging, layer caches, and overlay2 storage wear on flash storage.
* **Instant Start:** Managed by `systemd`, the binary starts in **< 20 milliseconds**.

### Unified Task Automation (`Justfile`)

```makefile
# Run backend locally on Windows in release mode (or watch with auto-reload)
run:
    cargo run --release

dev:
    cargo watch -x "run --release"

# Full local release build
build-local:
    cargo build --release

# Prepare offline SQLx cache (required before cross-compilation)
sqlx-prepare:
    cargo sqlx prepare

# Cross-compile for Linux (e.g. 64-bit ARM Linux musl static binary)
build-linux: sqlx-prepare
    cross build --target aarch64-unknown-linux-musl --release

# Deploy single executable and prod override to remote Linux host
deploy-linux REMOTE_HOST:
    scp target/aarch64-unknown-linux-musl/release/ultra-engine {{REMOTE_HOST}}:/opt/myapp/ultra-engine
    scp config/default.yaml {{REMOTE_HOST}}:/opt/myapp/config/default.yaml
    scp config/prod.yaml {{REMOTE_HOST}}:/opt/myapp/config/prod.yaml
    ssh {{REMOTE_HOST}} "sudo systemctl restart myapp"
```

### Production `systemd` Service (`/etc/systemd/system/myapp.service`)

```ini
[Unit]
Description=Ultra Engine Backend Service
After=network.target

[Service]
Type=simple
User=pi
WorkingDirectory=/opt/myapp
ExecStart=/opt/myapp/ultra-engine --config-override config/prod.yaml
Restart=always
RestartSec=5
Environment=RUST_LOG=info

# Hardening
ProtectSystem=full
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```