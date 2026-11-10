use std::convert::Infallible;
use std::time::Duration;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Redirect, Response, Sse,
    },
    Json,
};
use futures::Stream;
use serde::Deserialize;
use serde_json::json;
use serenity::all::{ChannelType, GuildId, UserId};
use tracing::{debug, error, info, warn};

use crate::api::AppState;
use crate::db;
use crate::models::{
    AIAutoQueueConfig, AIBatchConfirmRequest, AISearchRequest, AddPlaylistRequest, AddSongRequest,
    ContributeKeyRequest, DiscordGuild, DiscordUser, GuildSession,
    IndexRequest, PlaylistPreviewRequest,
    ServerChannel, ServerMember, Song, UserSession, VolumeRequest,
};

#[derive(Debug, Deserialize)]
pub struct AuthQuery {
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteOccurrenceQuery {
    pub occurrence: Option<String>,
}

pub async fn extract_session(
    headers: &HeaderMap,
    query: Option<&AuthQuery>,
    state: &AppState,
) -> Result<UserSession, (StatusCode, Json<serde_json::Value>)> {
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

// ---------------------------------------------------------------------------
// OAuth & Auth
// ---------------------------------------------------------------------------

pub async fn login(State(state): State<AppState>) -> impl IntoResponse {
    let url = format!(
        "https://discord.com/api/oauth2/authorize?client_id={}&redirect_uri={}&response_type=code&scope=identify%20guilds",
        state.config.discord_client_id,
        url::form_urlencoded::byte_serialize(state.config.discord_redirect_uri.as_bytes()).collect::<String>()
    );
    debug!("[Auth] /login invoked, redirecting to Discord: {}", url);
    Redirect::temporary(&url)
}

pub async fn callback(
    Query(q): Query<CallbackQuery>,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    debug!("[Auth] /callback invoked with query: {:?}", q);
    let code = match q.code {
        Some(c) if !c.is_empty() => c,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "No code provided" })),
            ))
        }
    };

    let params = [
        ("client_id", state.config.discord_client_id.as_str()),
        ("client_secret", state.config.discord_client_secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", state.config.discord_redirect_uri.as_str()),
    ];

    let token_res = match state
        .http_client
        .post("https://discord.com/api/oauth2/token")
        .form(&params)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": format!("Discord token request failed: {}", e) })),
            ))
        }
    };

    if !token_res.status().is_success() {
        let err_body = token_res.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "detail": format!("OAuth failed: {}", err_body) })),
        ));
    }

    let token_val: serde_json::Value = token_res.json().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": e.to_string() })),
        )
    })?;

    let access_token = token_val
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // Fetch @me
    let user_res = state
        .http_client
        .get("https://discord.com/api/users/@me")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": e.to_string() })),
            )
        })?;

    let user: DiscordUser = user_res.json().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": e.to_string() })),
        )
    })?;

    // Fetch guilds
    let guilds_res = state
        .http_client
        .get("https://discord.com/api/users/@me/guilds")
        .bearer_auth(access_token)
        .header("User-Agent", "MusicBot/1.0")
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": e.to_string() })),
            )
        })?;

    let guilds: Vec<DiscordGuild> = match guilds_res.json::<Vec<DiscordGuild>>().await {
        Ok(g) => {
            debug!("[Auth] Successfully fetched {} user guilds from Discord", g.len());
            g
        }
        Err(e) => {
            warn!("[Auth] Failed to deserialize @me/guilds: {}", e);
            vec![]
        }
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db::save_session(&state.pool, &session_id, &user, &guilds, access_token).await {
        error!("[Auth] Failed to save session to SQLite: {}", e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("Failed to save session: {}", e) })),
        ));
    }
    debug!("[Auth] Created session {} for user {} ({})", session_id, user.username, user.id);

    let separator = if state.config.frontend_url.contains('?') { "&" } else { "?" };
    let redirect_url = format!("{}{}{}", state.config.frontend_url, separator, format!("token={}", session_id));
    Ok(Redirect::temporary(&redirect_url).into_response())
}

pub async fn get_me(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    Ok(Json(json!({ "user": session.user })))
}

pub async fn logout(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if let Ok(session) = extract_session(&headers, Some(&q), &state).await {
        let _ = db::delete_session(&state.pool, &session.session_id).await;
    }
    Ok(Json(json!({ "status": "success" })))
}

// ---------------------------------------------------------------------------
// Guilds & Voice Status
// ---------------------------------------------------------------------------

pub async fn get_guilds(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let user_id_u64: u64 = session.user.id.parse().unwrap_or(0);
    debug!("[API] /api/guilds invoked by user {} ({})", session.user.username, session.user.id);
    let mut guild_list = Vec::new();

    let user_guild_map: std::collections::HashMap<_, _> = session
        .guilds
        .iter()
        .map(|g| (g.id.clone(), g.clone()))
        .collect();

    // Inspect guilds from Serenity Cache
    let cached_guilds = state.serenity_cache.guilds();
    debug!("[API] User has {} guilds in session; Serenity cache has {} guilds", user_guild_map.len(), cached_guilds.len());
    for g_id in cached_guilds {
        let g_id_str = g_id.to_string();
        let user_guild_opt = user_guild_map.get(&g_id_str);

        // Show guild if user is confirmed in it, or if user_guild_map is empty (fallback to bot's guilds)
        if user_guild_opt.is_some() || user_guild_map.is_empty() {
            let bot_connected = state.voice_mgr.is_connected(g_id.get()).await;
            let mut user_channel_id = None;
            let mut user_channel_name = None;
            let mut guild_name = user_guild_opt.map(|g| g.name.clone()).unwrap_or_else(|| "Discord Server".to_string());
            let mut guild_icon = user_guild_opt.and_then(|g| g.icon.clone());
            let is_owner = user_guild_opt.and_then(|g| g.owner).unwrap_or(false);

            if let Some(guild) = state.serenity_cache.guild(g_id) {
                guild_name = guild.name.clone();
                if guild_icon.is_none() {
                    guild_icon = guild.icon.map(|ic| ic.to_string());
                }
                if let Some(vs) = guild.voice_states.get(&UserId::new(user_id_u64)) {
                    if let Some(c_id) = vs.channel_id {
                        user_channel_id = Some(c_id.to_string());
                        if let Some(ch) = guild.channels.get(&c_id) {
                            user_channel_name = Some(ch.name.clone());
                        }
                    }
                }
            }

            let is_admin = session.user.id == state.config.admin_user_id || is_owner;

            guild_list.push(GuildSession {
                id: g_id_str,
                name: guild_name,
                icon: guild_icon.map(|ic| {
                    if ic.starts_with("http") {
                        ic
                    } else {
                        format!("https://cdn.discordapp.com/icons/{}/{}.png", g_id, ic)
                    }
                }),
                channel_id: user_channel_id,
                channel_name: user_channel_name,
                bot_connected,
                is_admin,
            });
        }
    }

    // Fallback: If cache was empty or bot is starting up, include user guilds directly
    if guild_list.is_empty() {
        for g in session.guilds {
            let gid_u64: u64 = g.id.parse().unwrap_or(0);
            let bot_connected = state.voice_mgr.is_connected(gid_u64).await;
            let is_admin = session.user.id == state.config.admin_user_id || g.owner.unwrap_or(false);
            guild_list.push(GuildSession {
                id: g.id.clone(),
                name: g.name.clone(),
                icon: g.icon.map(|ic| format!("https://cdn.discordapp.com/icons/{}/{}.png", g.id, ic)),
                channel_id: None,
                channel_name: None,
                bot_connected,
                is_admin,
            });
        }
    }

    Ok(Json(json!({ "guilds": guild_list })))
}

pub async fn get_voice_status(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    get_guilds(headers, Query(q), State(state)).await
}

// ---------------------------------------------------------------------------
// Queue & Streaming
// ---------------------------------------------------------------------------

pub async fn get_queue(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let queue_state = state.voice_mgr.get_queue_state(gid_u64).await;
    Ok(Json(queue_state))
}

pub async fn ensure_voice_connected(
    state: &AppState,
    session_user_id: &str,
    guild_id: u64,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if state.voice_mgr.is_connected(guild_id).await {
        return Ok(());
    }

    let uid_u64: u64 = session_user_id.parse().unwrap_or(0);
    let mut target_channel = None;
    if let Some(guild) = state.serenity_cache.guild(GuildId::new(guild_id)) {
        if let Some(vs) = guild.voice_states.get(&UserId::new(uid_u64)) {
            target_channel = vs.channel_id;
        }
    }

    let channel_id = match target_channel {
        Some(cid) => cid.get(),
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "You must be in a voice channel first to play music" })),
            ));
        }
    };

    info!("[Voice] Auto-summoning bot to user voice channel {} in guild {}", channel_id, guild_id);
    if let Err(e) = state.voice_mgr.join_channel(guild_id, channel_id).await {
        error!("[Voice] Auto-join failed: {}", e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("Failed to join voice channel: {}", e) })),
        ));
    }

    Ok(())
}

pub async fn add_song(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<AddSongRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    ensure_voice_connected(&state, &session.user.id, gid_u64).await?;

    let requester_name = session
        .user
        .global_name
        .clone()
        .unwrap_or_else(|| session.user.username.clone());
    let requester_avatar = session
        .user
        .avatar
        .as_ref()
        .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", session.user.id, a));

    let song = match crate::services::youtube::extract_song_metadata(
        &req.url,
        Some(session.user.id.clone()),
        Some(requester_name),
        requester_avatar,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": format!("Failed to extract track metadata: {}", e) })),
            ))
        }
    };

    if let Err(e) = state.voice_mgr.enqueue_song(gid_u64, song).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("Failed to queue track: {}", e) })),
        ));
    }

    Ok(Json(json!({ "status": "success" })))
}

pub async fn stream_queue(
    Path(guild_id): Path<String>,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&HeaderMap::new(), Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    let stream = async_stream::stream! {
        loop {
            let current_state = state.voice_mgr.get_queue_state(gid_u64).await;
            if let Ok(json_str) = serde_json::to_string(&current_state) {
                yield Ok(Event::default().data(json_str));
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

// ---------------------------------------------------------------------------
// Playback Controls
// ---------------------------------------------------------------------------

pub async fn control_skip(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.skip(gid_u64).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_skipto(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<IndexRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.skipto(gid_u64, req.index as usize).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_remove(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<IndexRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.remove(gid_u64, req.index as usize).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_clear(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.clear(gid_u64).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_join(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    ensure_voice_connected(&state, &session.user.id, gid_u64).await?;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_stop(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.stop(gid_u64).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_pause(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.pause(gid_u64).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_resume(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.resume(gid_u64).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_loop(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let cur_state = state.voice_mgr.get_queue_state(gid_u64).await;
    let _ = state.voice_mgr.set_loop(gid_u64, !cur_state.loop_enabled).await;
    Ok(Json(json!({ "status": "success" })))
}

pub async fn control_volume(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<VolumeRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let _ = state.voice_mgr.set_volume(gid_u64, req.volume).await;
    Ok(Json(json!({ "status": "success" })))
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

pub async fn get_lyrics(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let queue_state = state.voice_mgr.get_queue_state(gid_u64).await;

    let current = match queue_state.current {
        Some(s) => s,
        None => return Ok(Json(json!({ "lyrics": null, "error": "Nothing currently playing" }))),
    };

    let artist = current.artist.unwrap_or_default();
    let video_id = crate::services::lyrics::extract_video_id(&current.url);
    let lyrics = state
        .lyrics_svc
        .get_lyrics(&current.title, &artist, current.duration, video_id.as_deref())
        .await;
    Ok(Json(json!({ "lyrics": lyrics })))
}

// ---------------------------------------------------------------------------
// Playlist Preview & Batch Add
// ---------------------------------------------------------------------------

pub async fn playlist_preview(
    Path(_guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<PlaylistPreviewRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    match crate::services::youtube::preview_playlist(&req.url).await {
        Ok(res) => Ok(Json(json!(res))),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "detail": format!("Failed to preview playlist: {}", e) })),
        )),
    }
}

pub async fn playlist_add(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<AddPlaylistRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    ensure_voice_connected(&state, &session.user.id, gid_u64).await?;

    let requester_name = session
        .user
        .global_name
        .clone()
        .unwrap_or_else(|| session.user.username.clone());
    let requester_avatar = session
        .user
        .avatar
        .as_ref()
        .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", session.user.id, a));

    let mut songs = Vec::new();
    if let Some(track_reqs) = req.tracks {
        let start = req.start_index.unwrap_or(1).max(1) as usize;
        let end = req.end_index.map(|e| e as usize).unwrap_or(track_reqs.len());

        for (i, t) in track_reqs.into_iter().enumerate() {
            let track_idx = i + 1;
            if track_idx >= start && track_idx <= end {
                songs.push(Song {
                    url: t.url,
                    title: t.title.unwrap_or_else(|| format!("Track {}", track_idx)),
                    artist: t.artist,
                    thumbnail: t.thumbnail,
                    duration: t.duration.unwrap_or(0.0),
                    requester_id: Some(session.user.id.clone()),
                    requester_name: Some(requester_name.clone()),
                    requester_avatar: requester_avatar.clone(),
                    requester_handle: Some(requester_name.clone()),
                    queue_id: Some(uuid::Uuid::new_v4().to_string()),
                });
            }
        }
    } else if let Some(url) = req.url {
        let preview = crate::services::youtube::preview_playlist(&url)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "detail": e.to_string() }))))?;

        let start = req.start_index.unwrap_or(1).max(1) as usize;
        let end = req.end_index.map(|e| e as usize).unwrap_or(preview.tracks.len());

        for t in preview.tracks {
            let idx = t.index as usize;
            if idx >= start && idx <= end {
                songs.push(Song {
                    url: t.url,
                    title: t.title,
                    artist: Some(t.artist),
                    thumbnail: Some(t.thumbnail),
                    duration: t.duration,
                    requester_id: Some(session.user.id.clone()),
                    requester_name: Some(requester_name.clone()),
                    requester_avatar: requester_avatar.clone(),
                    requester_handle: Some(requester_name.clone()),
                    queue_id: Some(uuid::Uuid::new_v4().to_string()),
                });
            }
        }
    }

    let added_count = state.voice_mgr.enqueue_batch(gid_u64, songs).await.unwrap_or(0);
    Ok(Json(json!({
        "status": "success",
        "added_count": added_count,
        "playlist_title": req.title.unwrap_or_else(|| "YouTube Playlist".to_string())
    })))
}

// ---------------------------------------------------------------------------
// AI Auto-Queue & Keys
// ---------------------------------------------------------------------------

pub async fn ai_suggest(
    Path(_guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<AISearchRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let queries = match crate::services::ai::generate_search_queries(
        &state.http_client,
        &state.pool,
        &req.prompt,
        (req.count * 2) as usize,
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "detail": format!("AI service error: {}", e) })),
            ))
        }
    };

    let songs = crate::services::ai::search_songs(
        &queries,
        req.count as usize,
        &std::collections::HashSet::new(),
    )
    .await;

    Ok(Json(json!({
        "songs": songs,
        "prompt": req.prompt
    })))
}

pub async fn ai_confirm(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<AddSongRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    add_song(Path(guild_id), headers, Query(q), State(state), Json(req)).await
}

pub async fn ai_confirm_batch(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<AIBatchConfirmRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    ensure_voice_connected(&state, &session.user.id, gid_u64).await?;

    let requester_name = session
        .user
        .global_name
        .clone()
        .unwrap_or_else(|| session.user.username.clone());
    let requester_avatar = session
        .user
        .avatar
        .as_ref()
        .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", session.user.id, a));

    let songs: Vec<Song> = req
        .songs
        .into_iter()
        .map(|s| Song {
            url: s.url,
            title: s.title.unwrap_or_else(|| "YouTube Track".to_string()),
            artist: s.artist,
            thumbnail: s.thumbnail,
            duration: s.duration.unwrap_or(0.0),
            requester_id: Some(session.user.id.clone()),
            requester_name: Some(requester_name.clone()),
            requester_avatar: requester_avatar.clone(),
            requester_handle: Some(requester_name.clone()),
            queue_id: Some(uuid::Uuid::new_v4().to_string()),
        })
        .collect();

    let added = state.voice_mgr.enqueue_batch(gid_u64, songs).await.unwrap_or(0);
    Ok(Json(json!({ "status": "success", "added_count": added })))
}

pub async fn get_autoqueue(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let state_arc = state.voice_mgr.get_or_create_state(gid_u64).await;
    let s = state_arc.read().await;

    Ok(Json(json!({
        "enabled": s.ai_auto_enabled,
        "prompt": s.ai_prompt,
        "count": 5,
        "threshold": 2,
        "set_by_name": s.ai_set_by_name,
        "set_by_avatar": s.ai_set_by_avatar
    })))
}

pub async fn set_autoqueue(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(mut cfg): Json<AIAutoQueueConfig>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);

    if cfg.enabled {
        ensure_voice_connected(&state, &session.user.id, gid_u64).await?;
    }

    cfg.set_by_name = Some(session.user.username.clone());
    cfg.set_by_avatar = session.user.avatar.map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", session.user.id, a));

    let _ = db::save_autoqueue_config(&state.pool, &guild_id, &cfg).await;

    let state_arc = state.voice_mgr.get_or_create_state(gid_u64).await;
    {
        let mut s = state_arc.write().await;
        s.ai_auto_enabled = cfg.enabled;
        s.ai_prompt = cfg.prompt.clone();
        s.ai_set_by_name = cfg.set_by_name.clone();
        s.ai_set_by_avatar = cfg.set_by_avatar.clone();
    }

    if cfg.enabled && !cfg.prompt.is_empty() {
        state.voice_mgr.check_autoqueue(gid_u64).await;
    }

    Ok(Json(json!({ "status": "ok", "config": cfg })))
}

pub async fn list_gemini_keys(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let keys = db::get_all_gemini_keys_for_display(&state.pool).await;
    let total_active = keys.iter().filter(|k| k.is_active && k.status == "healthy").count();
    let is_admin = session.user.id == state.config.admin_user_id;

    Ok(Json(json!({
        "keys": keys,
        "total_active": total_active,
        "total_keys": keys.len(),
        "is_admin": is_admin
    })))
}

pub async fn contribute_gemini_key(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(req): Json<ContributeKeyRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let key = req.api_key.trim();
    if key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "API key is required" })),
        ));
    }

    let probe = crate::services::ai::verify_gemini_api_key(&state.http_client, key).await;
    let is_active = probe.status == "healthy" || probe.status == "quota_exceeded";

    let key_id = db::add_gemini_key(
        &state.pool,
        key,
        &session.user.id,
        &session.user.username,
        session.user.avatar.as_deref(),
        &probe.status,
        &probe.message,
        probe.retry_delay,
        is_active,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "detail": e.to_string() }))))?;

    Ok(Json(json!({
        "status": "success",
        "key_status": probe.status,
        "message": probe.message,
        "key_id": key_id
    })))
}

pub async fn remove_gemini_key(
    Path(key_id): Path<i64>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    if session.user.id != state.config.admin_user_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "detail": "Only administrators can delete keys" })),
        ));
    }

    let deleted = db::delete_gemini_key(&state.pool, key_id).await.unwrap_or(false);
    if !deleted {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "detail": "Key not found" })),
        ));
    }

    Ok(Json(json!({ "status": "success" })))
}

// ---------------------------------------------------------------------------
// Schedule & Guild Metadata
// ---------------------------------------------------------------------------

async fn fetch_guild_members_list(state: &AppState, gid: GuildId) -> Vec<ServerMember> {
    let mut members = Vec::new();

    // 1. Fetch live member list via Discord REST API (up to 1000 members)
    match state.discord_http.get_guild_members(gid, Some(1000), None).await {
        Ok(fetched) => {
            for m in fetched {
                if !m.user.bot {
                    members.push(ServerMember {
                        id: m.user.id.to_string(),
                        name: m.nick.unwrap_or(m.user.name),
                        avatar: m
                            .user
                            .avatar
                            .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", m.user.id, a))
                            .unwrap_or_default(),
                    });
                }
            }
        }
        Err(e) => {
            warn!("[API] Failed to fetch guild members via HTTP for guild {}: {}", gid, e);
        }
    }

    // 2. Fallback to serenity cache if REST API had an error or returned empty
    if members.is_empty() {
        if let Some(guild) = state.serenity_cache.guild(gid) {
            for (uid, m) in &guild.members {
                if !m.user.bot {
                    members.push(ServerMember {
                        id: uid.to_string(),
                        name: m.nick.clone().unwrap_or_else(|| m.user.name.clone()),
                        avatar: m
                            .user
                            .avatar
                            .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", uid, a))
                            .unwrap_or_default(),
                    });
                }
            }
        }
    }

    members
}

async fn fetch_guild_voice_channels_list(state: &AppState, gid: GuildId) -> Vec<ServerChannel> {
    let mut channels = Vec::new();

    match state.discord_http.get_channels(gid).await {
        Ok(fetched) => {
            for ch in fetched {
                if ch.kind == ChannelType::Voice || ch.kind == ChannelType::Stage {
                    channels.push(ServerChannel {
                        id: ch.id.to_string(),
                        name: ch.name.clone(),
                        r#type: if ch.kind == ChannelType::Stage {
                            "stage".to_string()
                        } else {
                            "voice".to_string()
                        },
                    });
                }
            }
        }
        Err(e) => {
            warn!("[API] Failed to fetch channels via HTTP for guild {}: {}", gid, e);
        }
    }

    if channels.is_empty() {
        if let Some(guild) = state.serenity_cache.guild(gid) {
            for (cid, ch) in &guild.channels {
                if ch.kind == ChannelType::Voice || ch.kind == ChannelType::Stage {
                    channels.push(ServerChannel {
                        id: cid.to_string(),
                        name: ch.name.clone(),
                        r#type: if ch.kind == ChannelType::Stage {
                            "stage".to_string()
                        } else {
                            "voice".to_string()
                        },
                    });
                }
            }
        }
    }

    channels
}

pub async fn get_guild_members(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let gid = GuildId::new(gid_u64);
    let members = fetch_guild_members_list(&state, gid).await;

    Ok(Json(json!({ "members": members })))
}

pub async fn get_guild_channels(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let gid = GuildId::new(gid_u64);
    let channels = fetch_guild_voice_channels_list(&state, gid).await;

    Ok(Json(json!({ "channels": channels })))
}

pub async fn get_schedule(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let gid_u64: u64 = guild_id.parse().unwrap_or(0);
    let gid = GuildId::new(gid_u64);

    let events = db::get_schedule_events(&state.pool, &guild_id).await;
    let members = fetch_guild_members_list(&state, gid).await;
    let channels = fetch_guild_voice_channels_list(&state, gid).await;

    Ok(Json(json!({
        "events": events,
        "members": members,
        "channels": channels
    })))
}

pub async fn create_schedule(
    Path(guild_id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let avatar = session
        .user
        .avatar
        .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", session.user.id, a));
    let organizer_name = session
        .user
        .global_name
        .clone()
        .unwrap_or_else(|| session.user.username.clone());

    match db::create_schedule_event(
        &state.pool,
        &guild_id,
        payload,
        &session.user.id,
        &organizer_name,
        avatar.as_deref(),
    )
    .await
    {
        Ok(ev) => Ok(Json(json!({ "status": "ok", "event": ev }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("Failed to create schedule: {}", e) })),
        )),
    }
}

pub async fn update_schedule(
    Path((_guild_id, event_id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let is_admin = session.user.id == state.config.admin_user_id;

    match db::update_schedule_event(&state.pool, &event_id, &payload, &session.user.id, is_admin).await {
        Ok(Some(ev)) => Ok(Json(json!({ "status": "ok", "event": ev }))),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(json!({ "detail": "Event not found" })))),
        Err(e) => Err((StatusCode::FORBIDDEN, Json(json!({ "detail": e.to_string() })))),
    }
}

pub async fn delete_schedule(
    Path((_guild_id, event_id)): Path<(String, String)>,
    Query(del_q): Query<DeleteOccurrenceQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, None, &state).await?;
    let is_admin = session.user.id == state.config.admin_user_id;

    match db::delete_schedule_event(
        &state.pool,
        &event_id,
        &session.user.id,
        is_admin,
        del_q.occurrence.as_deref(),
    )
    .await
    {
        Ok(true) => Ok(Json(json!({ "status": "ok" }))),
        Ok(false) => Err((StatusCode::NOT_FOUND, Json(json!({ "detail": "Event not found" })))),
        Err(e) => Err((StatusCode::FORBIDDEN, Json(json!({ "detail": e.to_string() })))),
    }
}

pub async fn respond_schedule(
    Path((_guild_id, event_id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let session = extract_session(&headers, Some(&q), &state).await?;
    let key = payload.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("accepted");
    let scope = payload.get("scope").and_then(|v| v.as_str()).unwrap_or("occurrence");

    match db::respond_schedule_event(&state.pool, &event_id, &session.user.id, key, status, scope).await {
        Some(ev) => Ok(Json(json!({ "status": "ok", "event": ev }))),
        None => Err((StatusCode::NOT_FOUND, Json(json!({ "detail": "Event not found" })))),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

pub async fn get_logs(
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let _ = extract_session(&headers, Some(&q), &state).await?;
    let log_candidates = ["logs/musicbot.log", "logs/start.log", "logs/bot_runner.log"];
    let mut logs = "No log file found.".to_string();

    for path in log_candidates {
        if std::path::Path::new(path).exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                let lines: Vec<&str> = content.lines().collect();
                let tail_count = lines.len().min(200);
                logs = lines[lines.len() - tail_count..].join("\n");
                break;
            }
        }
    }

    Ok(logs)
}
