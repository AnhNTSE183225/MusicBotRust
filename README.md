For AI Agents: Must follow https://github.com/AnhNTSE183225/techstack/blob/main/VIBE_CODING_RULES.md

# MusicBot

High-performance Rust backend engine for **MusicBỏTúi** — a Discord voice music bot and real-time web/mobile control API built with **Axum**, **Serenity**, **Songbird**, **Symphonia**, and **SQLite (`sqlx`)**.

---

## Architecture & Key Features

* **Deterministic Playback State Machine (`src/voice/state.rs`, `src/voice/manager.rs`)**
  * Unified `PlaybackStatus` lifecycle (`disconnected` → `idle` → `loading` → `playing` → `paused`) streamed in real time to Web (`MusicBotUI`) and Mobile (`MusicBotMobile`) clients over SSE (`/api/stream/{guild_id}`).
  * Preserves queue cursor indices across idle queue completions, loop mode, skips, clears, and track removals, and broadcasts structured `playback_error` + per-track `error` reasons when a video is unavailable.
* **Low-Latency YouTube Streaming & Cascade Fallbacks (`src/services/youtube.rs`)**
  * Overlaps Discord voice join (`ensure_voice_connected`) with `yt-dlp` metadata + direct stream URL resolution (`tokio::join!`) and streams raw PCM directly through `ffmpeg` + `prebuffer_audio`.
  * Automatically cascades across `yt-dlp` player client strategies (`android,mweb,web` → `default` → `tv,mweb,web`) if a stream returns HTTP `403 Forbidden` or empty audio bytes.
  * Paginated playlist & YouTube Mix (`RD...`) extraction (`offset` + `limit=20`, `20s` timeout guard) so large or infinite radio playlists open quickly with incremental loading.
* **AI Auto-Queue & Community Key Pool (`src/services/ai.rs`)**
  * Powered by Google Gemini (`GEMINI_MODEL` / `ai.model`, defaulting to `gemini-3.8-flash`) with automatic `HTTP 404` model discovery via `/v1beta/models` + 6-hour caching if a model is deprecated.
  * Round-robin community Gemini API key pool stored in SQLite with health probing and automatic cooldown handling (`429` / `400` / `403`).
* **Synchronized Lyrics & Discord Server Schedule (`src/services/lyrics.rs`, `src/services/schedule.rs`)**
  * Fetches synced / syllable-level lyrics and manages recurring Discord server schedule events, RSVPs, and automated Discord reminders.
* **Decoupled Production Observability (`systemd-journald` + `VictoriaLogs`)**
  * Structured JSON logging (`tracing-subscriber`) with `duration_ms` latency spans, panic hooks with debug symbols (`strip = "debuginfo"` + `RUST_BACKTRACE=1`), and inherited subprocess `stderr` shipped cleanly from `journald` to local `VictoriaLogs`.

---

## Project Layout

```text
MusicBot/
├── Cargo.toml                 # Dependencies & release profile (strip = "debuginfo", lto = "thin")
├── .env                       # Environment profile selector (PROFILE=DEV or PROD) & secrets
├── config.dev.yaml            # Development configuration
├── config.prod.yaml           # Production configuration
├── build.ps1 / build.sh       # Production release build scripts (Windows / Linux)
├── start.ps1 / start.sh       # Production runners (DEV manual / PROD systemd + Tailscale)
├── stop.ps1 / stop.sh         # Deterministic process / service stoppers
├── deploy/systemd/
│   └── musicbot.service       # Linux systemd unit (journald + RUST_BACKTRACE=1)
└── src/
    ├── main.rs                # Entrypoint, tracing setup, panic hook, service startup
    ├── config.rs              # Profile-driven YAML + .env configuration loader
    ├── models.rs              # Shared API, queue state, playlist, lyrics & schedule DTOs
    ├── db.rs                  # SQLite WAL initialization, sessions, AI keys & schedule store
    ├── api/                   # Axum router, HTTP latency middleware & REST/SSE handlers
    ├── services/              # yt-dlp/YouTube, Gemini AI, lyrics & schedule services
    └── voice/                 # Songbird voice manager & GuildMusicState machine
```

---

## Prerequisites

* **Rust toolchain** (`rustc` / `cargo`)
* **`ffmpeg`** on `PATH`
* **`yt-dlp`** on `PATH` (keep updated regularly for YouTube extractor compatibility)
* **Linux PROD host (optional):** `systemd`, `systemd-journal-remote` (`systemd-journal-upload`), and standalone `VictoriaLogs` — see [Observability Setup](#production-observability-systemd--victorialogs)

---

## Configuration (`.env` + Profile YAML)

`AppConfig::load()` reads `.env` and selects either `config.dev.yaml` or `config.prod.yaml` based on `PROFILE` / `DISCORD_PROFILE`:

| Variable | Purpose |
| :--- | :--- |
| `PROFILE` / `DISCORD_PROFILE` | `DEV` (uses `DEV_DISCORD_*` + `config.dev.yaml`) or `PROD` (uses `DISCORD_*` + `config.prod.yaml`) |
| `DISCORD_TOKEN` / `DISCORD_CLIENT_ID` / `DISCORD_CLIENT_SECRET` | Production Discord bot & OAuth2 credentials |
| `DEV_DISCORD_TOKEN` / `DEV_DISCORD_CLIENT_ID` / `DEV_DISCORD_CLIENT_SECRET` | Development Discord bot & OAuth2 credentials |
| `USER_ID` | Admin Discord user ID |
| `GEMINI_MODEL` | Optional override for `ai.model` in YAML (defaults to `gemini-3.8-flash`) |
| `CONFIG_PATH` | Optional explicit path overriding `config.dev.yaml` / `config.prod.yaml` |

---

## Build, Run & Test

### Windows (PowerShell)
```powershell
.\build.ps1       # Compile target\release\musicbot.exe
.\start.ps1       # Launch release binary
.\stop.ps1        # Stop running process cleanly
```

### Linux (Bash / Pi PROD)
```bash
./build.sh        # Compile target/release/musicbot
./start.sh        # Start systemd musicbot.service (+ Tailscale funnel if PROD)
./stop.sh         # Stop service cleanly
```

### Verification Tests
```bash
cargo check
cargo test --no-fail-fast
```

---

## Production Observability (`systemd` + `VictoriaLogs`)

Production runs under [`deploy/systemd/musicbot.service`](deploy/systemd/musicbot.service) with `StandardOutput=journal`, `StandardError=journal`, `SyslogIdentifier=musicbot`, and `RUST_BACKTRACE=1`.

1. **One-time host setup** (installs `VictoriaLogs` on `127.0.0.1:9428` and configures `systemd-journal-upload`):
   ```bash
   curl -fsSL https://raw.githubusercontent.com/AnhNTSE183225/techstack/main/observability/install.sh | sudo bash
   ```
2. **View logs locally via SSH tunnel** (from the `TECHSTACK` repo):
   ```powershell
   .\observability\open-logs.ps1
   ```
   Opens `http://localhost:9428/select/vmui/` in your browser without exposing port `9428` publicly.
3. **Direct journalctl fallback on the server:**
   ```bash
   journalctl -u musicbot -f
   ```

---

## Related Repositories

* **Web UI:** `MusicBotUI` (TanStack Start / React frontend deployed on Cloudflare Pages via `.\deploy.ps1`)
* **Mobile App:** `MusicBotMobile` (Flutter client)
* **Architecture & Rules:** [TECHSTACK](https://github.com/AnhNTSE183225/techstack) ([`VIBE_CODING_RULES.md`](https://github.com/AnhNTSE183225/techstack/blob/main/VIBE_CODING_RULES.md))
