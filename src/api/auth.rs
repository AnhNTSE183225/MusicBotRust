use axum::{
    extract::{FromRequestParts, Path, Query},
    http::{header, request::Parts, HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use serenity::all::{GuildId, UserId};
use tracing::warn;

use crate::api::AppState;
use crate::db;
use crate::models::UserSession;

pub type ApiRejection = (StatusCode, Json<serde_json::Value>);

#[derive(Debug, Deserialize)]
pub struct AuthQuery {
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GuildPathParam {
    guild_id: String,
}

/// Single source of truth for global bot administrator checks.
pub fn is_global_admin(state: &AppState, user_id: &str) -> bool {
    !state.config.admin_user_id.is_empty() && user_id == state.config.admin_user_id
}

/// Single source of truth for guild ownership checks across both Serenity's live cache
/// and the user's OAuth session guild list.
pub fn is_guild_owner(state: &AppState, session: &UserSession, guild_id: u64) -> bool {
    let uid_u64: u64 = session.user.id.parse().unwrap_or(0);
    let cache_owner = if uid_u64 > 0 && guild_id > 0 {
        state
            .serenity_cache
            .guild(GuildId::new(guild_id))
            .map(|g| g.owner_id == UserId::new(uid_u64))
            .unwrap_or(false)
    } else {
        false
    };

    if cache_owner {
        return true;
    }

    let guild_id_str = guild_id.to_string();
    session
        .guilds
        .iter()
        .any(|g| g.id == guild_id_str && g.owner.unwrap_or(false))
}

/// Single source of truth for guild administrator checks (global bot admin OR guild owner).
/// Shared by `/api/guilds` (`is_admin` UI flag), vote-bypass controls, schedule edits,
/// and the compile-time `GuildAdmin` extractor so UI and backend enforcement can never drift.
pub fn is_guild_admin(state: &AppState, session: &UserSession, guild_id: u64) -> bool {
    is_global_admin(state, &session.user.id) || is_guild_owner(state, session, guild_id)
}

pub async fn extract_session(
    headers: &HeaderMap,
    query: Option<&AuthQuery>,
    state: &AppState,
) -> Result<UserSession, ApiRejection> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.trim().to_string())
        .or_else(|| query.and_then(|q| q.token.clone()));

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "detail": "Missing authentication token" })),
            ))
        }
    };

    match db::get_session(&state.pool, &token).await {
        Some(sess) => Ok(sess),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "detail": "Invalid or expired session" })),
        )),
    }
}

/// Type-safe Axum extractor enforcing a valid authenticated session.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub UserSession);

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let query = Query::<AuthQuery>::from_request_parts(parts, state)
            .await
            .ok()
            .map(|q| q.0);
        let session = extract_session(&parts.headers, query.as_ref(), state).await?;
        Ok(Self(session))
    }
}

/// Type-safe Axum extractor enforcing global bot administrator privileges.
/// Handlers taking `GlobalAdmin` cannot execute unless the caller matches `config.admin_user_id`.
#[derive(Debug, Clone)]
pub struct GlobalAdmin(pub UserSession);

impl FromRequestParts<AppState> for GlobalAdmin {
    type Rejection = ApiRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthenticatedUser(session) =
            AuthenticatedUser::from_request_parts(parts, state).await?;

        if !is_global_admin(state, &session.user.id) {
            warn!(
                target: "auth_audit",
                user_id = %session.user.id,
                username = %session.user.username,
                path = %parts.uri.path(),
                "[Auth] Denied global-admin endpoint for non-admin user"
            );
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "detail": "Only administrators can perform this action" })),
            ));
        }

        Ok(Self(session))
    }
}

/// Type-safe Axum extractor enforcing guild administrator privileges for `{guild_id}` routes.
/// Handlers taking `GuildAdmin` cannot execute unless the caller is the global bot admin
/// or the owner of the target `{guild_id}` guild.
#[derive(Debug, Clone)]
pub struct GuildAdmin {
    pub session: UserSession,
    pub guild_id: u64,
    #[allow(dead_code)]
    pub guild_id_str: String,
}

impl FromRequestParts<AppState> for GuildAdmin {
    type Rejection = ApiRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthenticatedUser(session) =
            AuthenticatedUser::from_request_parts(parts, state).await?;

        let Path(GuildPathParam { guild_id: guild_id_str }) =
            Path::<GuildPathParam>::from_request_parts(parts, state)
                .await
                .map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "Missing or invalid guild_id path parameter" })),
                    )
                })?;

        let guild_id: u64 = guild_id_str.parse().map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "Invalid guild ID" })),
            )
        })?;

        if !is_guild_admin(state, &session, guild_id) {
            warn!(
                target: "auth_audit",
                user_id = %session.user.id,
                username = %session.user.username,
                guild_id = guild_id,
                path = %parts.uri.path(),
                "[Auth] Denied guild-admin endpoint for non-admin user"
            );
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "detail": "Only server administrators can perform this action" })),
            ));
        }

        Ok(Self {
            session,
            guild_id,
            guild_id_str,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use tower::ServiceExt;

    use crate::api::create_router;
    use crate::config::AppConfig;
    use crate::models::{DiscordGuild, DiscordUser};
    use crate::services::lyrics::LyricsService;
    use crate::services::schedule::ScheduleService;
    use crate::voice::manager::VoiceManager;

    async fn build_test_state() -> AppState {
        let db_url = format!(
            "sqlite:file:musicbot_auth_test_{}?mode=memory&cache=shared",
            uuid::Uuid::new_v4()
        );
        let pool = db::init_db(&db_url).await.expect("init in-memory db");

        let config = AppConfig {
            discord_token: "test-token".to_string(),
            discord_client_id: "test-client".to_string(),
            discord_client_secret: "test-secret".to_string(),
            discord_profile: "DEV".to_string(),
            admin_user_id: "admin-1".to_string(),
            host: "127.0.0.1".to_string(),
            port: 0,
            frontend_url: "http://localhost:3000".to_string(),
            cors_origins: vec!["*".to_string()],
            discord_redirect_uri: "http://localhost:8000/callback".to_string(),
            database_url: db_url,
            default_volume: 0.75,
            normalize_audio: false,
            log_level: "warn".to_string(),
            ai_model: "gemini-3.8-flash".to_string(),
        };

        let serenity_cache = Arc::new(serenity::all::Cache::default());
        let discord_http = Arc::new(serenity::all::Http::new("test-token"));
        let songbird = songbird::Songbird::serenity();
        let voice_mgr = Arc::new(VoiceManager::new(
            songbird,
            serenity_cache.clone(),
            pool.clone(),
            config.default_volume,
        ));
        let lyrics_svc = Arc::new(LyricsService::new());
        let schedule_svc = Arc::new(ScheduleService::new(
            pool.clone(),
            discord_http.clone(),
            config.frontend_url.clone(),
        ));

        AppState {
            config,
            pool,
            voice_mgr,
            lyrics_svc,
            schedule_svc,
            http_client: reqwest::Client::new(),
            discord_http,
            serenity_cache,
            mobile_auth_pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn seed_session(
        state: &AppState,
        token: &str,
        user_id: &str,
        username: &str,
        guilds: Vec<DiscordGuild>,
    ) {
        let user = DiscordUser {
            id: user_id.to_string(),
            username: username.to_string(),
            discriminator: None,
            global_name: Some(username.to_string()),
            avatar: None,
        };
        db::save_session(&state.pool, token, &user, &guilds, "discord-access")
            .await
            .expect("seed session");
    }

    fn volume_request(guild_id: &str, token: Option<&str>, volume: i32) -> Request<Body> {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/controls/{}/volume", guild_id))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", t));
        }
        builder
            .body(Body::from(json!({ "volume": volume }).to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn volume_control_rejects_unauthenticated_and_non_admin_users() {
        let state = build_test_state().await;
        seed_session(
            &state,
            "tok-regular",
            "user-999",
            "RegularListener",
            vec![DiscordGuild {
                id: "111".to_string(),
                name: "Music Room".to_string(),
                icon: None,
                owner: Some(false),
                permissions: None,
            }],
        )
        .await;

        let app = create_router(state.clone());

        // 1. Unauthenticated -> 401
        let resp = app
            .clone()
            .oneshot(volume_request("111", None, 10))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2. Authenticated regular user -> 403 Forbidden and volume unchanged
        let resp = app
            .clone()
            .oneshot(volume_request("111", Some("tok-regular"), 10))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let q = state.voice_mgr.get_queue_state(111).await;
        assert_eq!(q.volume, 75);
    }

    #[tokio::test]
    async fn volume_control_allows_global_admin_and_guild_owner_with_cross_guild_isolation() {
        let state = build_test_state().await;
        seed_session(&state, "tok-global-admin", "admin-1", "BotAdmin", vec![]).await;
        seed_session(
            &state,
            "tok-guild-owner",
            "owner-111",
            "GuildOwner",
            vec![DiscordGuild {
                id: "111".to_string(),
                name: "Guild 111".to_string(),
                icon: None,
                owner: Some(true),
                permissions: None,
            }],
        )
        .await;

        let app = create_router(state.clone());

        // 1. Global bot admin -> 200 OK
        let resp = app
            .clone()
            .oneshot(volume_request("111", Some("tok-global-admin"), 40))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.voice_mgr.get_queue_state(111).await.volume, 40);

        // 2. Owner of guild 111 -> 200 OK on guild 111
        let resp = app
            .clone()
            .oneshot(volume_request("111", Some("tok-guild-owner"), 55))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.voice_mgr.get_queue_state(111).await.volume, 55);

        // 3. Owner of guild 111 attempting to control guild 222 -> 403 Forbidden
        let resp = app
            .clone()
            .oneshot(volume_request("222", Some("tok-guild-owner"), 10))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(state.voice_mgr.get_queue_state(222).await.volume, 75);
    }

    #[tokio::test]
    async fn global_admin_extractor_blocks_non_admin_key_deletion() {
        let state = build_test_state().await;
        seed_session(&state, "tok-regular", "user-999", "RegularListener", vec![]).await;

        let app = create_router(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/ai/keys/1")
            .header(header::AUTHORIZATION, "Bearer tok-regular")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
