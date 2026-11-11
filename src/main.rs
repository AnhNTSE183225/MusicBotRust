mod api;
mod config;
mod db;
mod models;
mod services;
mod voice;

use std::net::SocketAddr;
use std::sync::Arc;
use serenity::all::{Client, GatewayIntents, Ready};
use serenity::async_trait;
use serenity::client::EventHandler;
use songbird::SerenityInit;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::AppState;
use crate::config::AppConfig;
use crate::services::lyrics::LyricsService;
use crate::services::schedule::ScheduleService;
use crate::voice::manager::VoiceManager;

struct BotEventHandler;

#[async_trait]
impl EventHandler for BotEventHandler {
    async fn ready(&self, _: serenity::client::Context, ready: Ready) {
        info!("[Discord] Gateway ready! Logged in as: {}#{}", ready.user.name, ready.user.discriminator.map(|d| d.to_string()).unwrap_or_default());
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Load Configuration
    let config = AppConfig::load()?;

    // 2. Initialize Tracing with profile-aware level
    let default_filter = if config.discord_profile == "DEV" || config.log_level.to_uppercase() == "DEBUG" {
        "musicbot=debug,tower_http=debug,axum=debug,songbird=info"
    } else {
        "musicbot=info,tower_http=info,songbird=info"
    };

    let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string());

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(env_filter))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting MusicBot (Rust Engine)...");
    info!("Active Discord Profile: {}", config.discord_profile);
    info!("Log Level: {}", config.log_level);
    info!("API Listening on {}:{}", config.host, config.port);

    // 3. Initialize SQLite Database
    let pool = db::init_db(&config.database_url).await?;
    info!("Database initialized at {}", config.database_url);

    // 4. Initialize Serenity + Songbird Client
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILD_MEMBERS;

    let songbird_manager = songbird::Songbird::serenity();

    let mut discord_client = Client::builder(&config.discord_token, intents)
        .event_handler(BotEventHandler)
        .register_songbird_with(songbird_manager.clone())
        .await?;

    let discord_http = discord_client.http.clone();
    let serenity_cache = discord_client.cache.clone();

    // 5. Instantiate Core Voice Manager & Services
    let voice_mgr = Arc::new(VoiceManager::new(
        songbird_manager,
        pool.clone(),
        config.default_volume,
    ));

    let lyrics_svc = Arc::new(LyricsService::new());
    let schedule_svc = Arc::new(ScheduleService::new(
        pool.clone(),
        discord_http.clone(),
        config.frontend_url.clone(),
    ));
    schedule_svc.clone().start_background_loop();

    // 6. Spawn Discord Gateway Task in Background
    tokio::spawn(async move {
        if let Err(e) = discord_client.start().await {
            error!("[Discord] Client runtime error: {:?}", e);
        }
    });

    // 7. Initialize and Start Axum Web Server
    let app_state = AppState {
        config: config.clone(),
        pool,
        voice_mgr,
        lyrics_svc,
        schedule_svc,
        http_client: reqwest::Client::builder().build().unwrap_or_default(),
        discord_http,
        serenity_cache,
    };

    let router = api::create_router(app_state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("[Server] Failed to bind to http://{}: {}. (Port {} may already be in use)", addr, e, config.port);
            return Err(e.into());
        }
    };
    info!("MusicBot HTTP Server running at http://{}", addr);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("MusicBot HTTP server completed serving. Main exiting cleanly.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => info!("[Shutdown] Ctrl+C signal received!"),
            Err(err) => error!("[Shutdown] Failed to listen for Ctrl+C signal: {}", err),
        }
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("[Shutdown] Shutdown triggered via Ctrl+C");
        },
        _ = terminate => {
            info!("[Shutdown] Shutdown triggered via SIGTERM");
        },
    }
    info!("Shutdown signal received, shutting down gracefully...");
}
