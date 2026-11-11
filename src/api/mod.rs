pub mod routes;

use std::sync::Arc;
use axum::{
    http::{header, Method},
    routing::{delete, get, post, put},
    Router,
};
use reqwest::Client;
use serenity::all::{Cache, Http};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use crate::config::AppConfig;
use crate::services::lyrics::LyricsService;
use crate::services::schedule::ScheduleService;
use crate::voice::manager::VoiceManager;

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub pool: SqlitePool,
    pub voice_mgr: Arc<VoiceManager>,
    pub lyrics_svc: Arc<LyricsService>,
    pub schedule_svc: Arc<ScheduleService>,
    pub http_client: Client,
    pub discord_http: Arc<Http>,
    pub serenity_cache: Arc<Cache>,
}

pub fn create_router(state: AppState) -> Router {
    // Build permissive CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::ORIGIN,
        ]);

    Router::new()
        .route("/login", get(routes::login))
        .route("/callback", get(routes::callback))
        .route("/api/me", get(routes::get_me))
        .route("/api/logout", post(routes::logout))
        .route("/api/guilds", get(routes::get_guilds))
        .route("/api/voice_status", get(routes::get_voice_status))
        .route("/api/queue/{guild_id}", get(routes::get_queue).post(routes::add_song))
        .route("/api/stream/{guild_id}", get(routes::stream_queue))
        .route("/api/controls/{guild_id}/skip", post(routes::control_skip))
        .route("/api/controls/{guild_id}/skipto", post(routes::control_skipto))
        .route("/api/controls/{guild_id}/remove", post(routes::control_remove))
        .route("/api/controls/{guild_id}/clear", post(routes::control_clear))
        .route("/api/controls/{guild_id}/join", post(routes::control_join))
        .route("/api/controls/{guild_id}/stop", post(routes::control_stop))
        .route("/api/controls/{guild_id}/pause", post(routes::control_pause))
        .route("/api/controls/{guild_id}/resume", post(routes::control_resume))
        .route("/api/controls/{guild_id}/loop", post(routes::control_loop))
        .route("/api/controls/{guild_id}/volume", post(routes::control_volume))
        .route("/api/lyrics/{guild_id}", get(routes::get_lyrics))
        .route("/api/playlist/{guild_id}/preview", post(routes::playlist_preview))
        .route("/api/playlist/{guild_id}/add", post(routes::playlist_add))
        .route("/api/ai/{guild_id}/suggest", post(routes::ai_suggest))
        .route("/api/ai/{guild_id}/confirm", post(routes::ai_confirm))
        .route("/api/ai/{guild_id}/confirm_batch", post(routes::ai_confirm_batch))
        .route("/api/ai/{guild_id}/autoqueue", get(routes::get_autoqueue).post(routes::set_autoqueue))
        .route("/api/ai/keys", get(routes::list_gemini_keys))
        .route("/api/ai/keys/contribute", post(routes::contribute_gemini_key))
        .route("/api/ai/keys/{key_id}", delete(routes::remove_gemini_key))
        .route("/api/guild/{guild_id}/members", get(routes::get_guild_members))
        .route("/api/guild/{guild_id}/channels", get(routes::get_guild_channels))
        .route("/api/schedule/{guild_id}", get(routes::get_schedule).post(routes::create_schedule))
        .route("/api/schedule/{guild_id}/{event_id}", put(routes::update_schedule).delete(routes::delete_schedule))
        .route("/api/schedule/{guild_id}/{event_id}/respond", post(routes::respond_schedule))
        .route("/api/logs", get(routes::get_logs))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
