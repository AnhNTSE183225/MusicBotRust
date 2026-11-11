use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use crate::models::{
    AIAutoQueueConfig, DiscordGuild, DiscordUser, GeminiKeyInfo, ScheduleEvent, UserSession,
};

pub fn now_sec() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn mask_key(key: &str) -> String {
    let k = key.trim();
    if k.len() <= 12 {
        "****".to_string()
    } else {
        format!("{}...{}", &k[..8], &k[k.len() - 4..])
    }
}

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = Path::new("data").canonicalize().ok() {
        let _ = fs::create_dir_all(parent);
    } else {
        let _ = fs::create_dir_all("data");
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await?;
    sqlx::query("PRAGMA synchronous=NORMAL;").execute(&pool).await?;
    sqlx::query("PRAGMA busy_timeout=10000;").execute(&pool).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            username TEXT NOT NULL,
            user_data TEXT NOT NULL,
            guilds_data TEXT NOT NULL,
            access_token TEXT NOT NULL,
            created_at REAL NOT NULL,
            expires_at REAL NOT NULL,
            last_active REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);

        CREATE TABLE IF NOT EXISTS gemini_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            api_key TEXT UNIQUE NOT NULL,
            added_by_user_id TEXT NOT NULL,
            added_by_username TEXT NOT NULL,
            added_by_avatar TEXT,
            added_at REAL NOT NULL,
            is_active INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'healthy',
            status_message TEXT DEFAULT '',
            cooldown_until REAL NOT NULL DEFAULT 0,
            total_uses INTEGER NOT NULL DEFAULT 0,
            last_used_at REAL NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_gemini_keys_status ON gemini_keys(is_active, status, cooldown_until);

        CREATE TABLE IF NOT EXISTS guild_music_state (
            guild_id TEXT PRIMARY KEY,
            state_json TEXT NOT NULL,
            updated_at REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS guild_autoqueue_config (
            guild_id TEXT PRIMARY KEY,
            prompt TEXT NOT NULL DEFAULT '',
            count INTEGER NOT NULL DEFAULT 5,
            threshold INTEGER NOT NULL DEFAULT 2,
            set_by_name TEXT,
            set_by_avatar TEXT,
            updated_at REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS guild_schedule_events (
            id TEXT PRIMARY KEY,
            guild_id TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            category TEXT NOT NULL DEFAULT 'other',
            start TEXT NOT NULL,
            duration_minutes INTEGER NOT NULL DEFAULT 60,
            recurrence_json TEXT,
            location_json TEXT,
            image_url TEXT,
            links_json TEXT NOT NULL DEFAULT '[]',
            organizer_id TEXT NOT NULL,
            organizer_name TEXT NOT NULL DEFAULT '',
            organizer_avatar TEXT,
            invite_everyone INTEGER NOT NULL DEFAULT 1,
            invitee_ids_json TEXT NOT NULL DEFAULT '[]',
            series_responses_json TEXT NOT NULL DEFAULT '{}',
            occurrence_responses_json TEXT NOT NULL DEFAULT '{}',
            cancelled_occurrences_json TEXT NOT NULL DEFAULT '[]',
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_guild_schedule_guild ON guild_schedule_events(guild_id);

        CREATE TABLE IF NOT EXISTS guild_schedule_reminders (
            event_id TEXT NOT NULL,
            occurrence_key TEXT NOT NULL,
            reminder_type TEXT NOT NULL DEFAULT 'lead',
            reminded_at REAL NOT NULL,
            PRIMARY KEY (event_id, occurrence_key, reminder_type)
        );

        CREATE TABLE IF NOT EXISTS user_permissions (
            user_id TEXT NOT NULL,
            permission_type TEXT NOT NULL,
            added_at REAL NOT NULL,
            reason TEXT DEFAULT '',
            PRIMARY KEY (user_id, permission_type)
        );

        CREATE TABLE IF NOT EXISTS bot_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at REAL NOT NULL
        );
        "#,
    )
    .execute(&pool)
    .await?;

    // Import from data/gemini_keys.txt if exists
    if Path::new("data/gemini_keys.txt").exists() {
        if let Ok(keys_content) = fs::read_to_string("data/gemini_keys.txt") {
            let now = now_sec();
            for line in keys_content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    let parts: Vec<&str> = trimmed.split('#').collect();
                    let key = parts[0].trim();
                    let user_id = parts.get(1).map(|s| s.trim()).unwrap_or("system");
                    let user_name = parts.get(2).map(|s| s.trim()).unwrap_or("System Contributor");
                    let _ = sqlx::query(
                        r#"
                        INSERT OR IGNORE INTO gemini_keys 
                        (api_key, added_by_user_id, added_by_username, added_at, is_active, status)
                        VALUES (?, ?, ?, ?, 1, 'healthy')
                        "#,
                    )
                    .bind(key)
                    .bind(user_id)
                    .bind(user_name)
                    .bind(now)
                    .execute(&pool)
                    .await;
                }
            }
        }
    }

    Ok(pool)
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

pub async fn save_session(
    pool: &SqlitePool,
    session_id: &str,
    user: &DiscordUser,
    guilds: &[DiscordGuild],
    access_token: &str,
) -> Result<(), sqlx::Error> {
    let now = now_sec();
    let expires = now + (30.0 * 86400.0);
    let user_json = serde_json::to_string(user).unwrap_or_default();
    let guilds_json = serde_json::to_string(guilds).unwrap_or_default();

    sqlx::query(
        r#"
        INSERT INTO sessions (session_id, user_id, username, user_data, guilds_data, access_token, created_at, expires_at, last_active)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            user_data = excluded.user_data,
            guilds_data = excluded.guilds_data,
            access_token = excluded.access_token,
            expires_at = excluded.expires_at,
            last_active = excluded.last_active
        "#,
    )
    .bind(session_id)
    .bind(&user.id)
    .bind(&user.username)
    .bind(user_json)
    .bind(guilds_json)
    .bind(access_token)
    .bind(now)
    .bind(expires)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_session(pool: &SqlitePool, session_id: &str) -> Option<UserSession> {
    let now = now_sec();
    let row = sqlx::query(
        "SELECT session_id, user_data, guilds_data, access_token, created_at, expires_at FROM sessions WHERE session_id = ? AND expires_at > ?",
    )
    .bind(session_id)
    .bind(now)
    .fetch_optional(pool)
    .await
    .ok()??;

    let user_data: String = row.get("user_data");
    let guilds_data: String = row.get("guilds_data");
    let user: DiscordUser = serde_json::from_str(&user_data).ok()?;
    let guilds: Vec<DiscordGuild> = serde_json::from_str(&guilds_data).unwrap_or_default();

    Some(UserSession {
        session_id: row.get("session_id"),
        user,
        guilds,
        access_token: row.get("access_token"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    })
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[allow(dead_code)]
pub async fn get_user_avatar(pool: &SqlitePool, user_id: &str) -> Option<String> {
    let row = sqlx::query("SELECT user_data FROM sessions WHERE user_id = ? ORDER BY last_active DESC LIMIT 1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()??;

    let user_data: String = row.get("user_data");
    let user: DiscordUser = serde_json::from_str(&user_data).ok()?;
    user.avatar.map(|av| format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", user.id, av))
}

// ---------------------------------------------------------------------------
// Gemini Keys
// ---------------------------------------------------------------------------

pub async fn get_all_gemini_keys_for_display(pool: &SqlitePool) -> Vec<GeminiKeyInfo> {
    let now = now_sec();
    let rows = sqlx::query(
        "SELECT id, api_key, added_by_user_id, added_by_username, added_by_avatar, added_at, is_active, status, status_message, cooldown_until, total_uses, last_used_at FROM gemini_keys ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(|r| {
            let raw_key: String = r.get("api_key");
            let cooldown_until: f64 = r.get("cooldown_until");
            let cooldown_rem = (cooldown_until - now).max(0.0);
            let is_active_int: i64 = r.get("is_active");

            GeminiKeyInfo {
                id: r.get("id"),
                masked_key: mask_key(&raw_key),
                added_by_user_id: r.get("added_by_user_id"),
                added_by_username: r.get("added_by_username"),
                added_by_avatar: r.get("added_by_avatar"),
                added_at: r.get("added_at"),
                is_active: is_active_int == 1,
                status: r.get("status"),
                status_message: r.get("status_message"),
                cooldown_remaining_seconds: cooldown_rem,
                total_uses: r.get("total_uses"),
                last_used_at: r.get("last_used_at"),
            }
        })
        .collect()
}

pub async fn add_gemini_key(
    pool: &SqlitePool,
    api_key: &str,
    user_id: &str,
    username: &str,
    avatar: Option<&str>,
    status: &str,
    status_msg: &str,
    cooldown_secs: f64,
    is_active: bool,
) -> Result<i64, sqlx::Error> {
    let now = now_sec();
    let cooldown_until = if cooldown_secs > 0.0 { now + cooldown_secs } else { 0.0 };

    let res = sqlx::query(
        r#"
        INSERT INTO gemini_keys (api_key, added_by_user_id, added_by_username, added_by_avatar, added_at, is_active, status, status_message, cooldown_until)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(api_key) DO UPDATE SET
            status = excluded.status,
            status_message = excluded.status_message,
            cooldown_until = excluded.cooldown_until,
            is_active = excluded.is_active
        "#,
    )
    .bind(api_key)
    .bind(user_id)
    .bind(username)
    .bind(avatar)
    .bind(now)
    .bind(if is_active { 1 } else { 0 })
    .bind(status)
    .bind(status_msg)
    .bind(cooldown_until)
    .execute(pool)
    .await?;

    Ok(res.last_insert_rowid())
}

pub async fn delete_gemini_key(pool: &SqlitePool, key_id: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM gemini_keys WHERE id = ?")
        .bind(key_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn pick_healthy_gemini_key(pool: &SqlitePool) -> Option<String> {
    let now = now_sec();
    let row = sqlx::query(
        r#"
        SELECT api_key FROM gemini_keys
        WHERE is_active = 1 AND (status = 'healthy' OR (status = 'quota_exceeded' AND cooldown_until <= ?))
        ORDER BY last_used_at ASC, total_uses ASC
        LIMIT 1
        "#,
    )
    .bind(now)
    .fetch_optional(pool)
    .await
    .ok()??;

    Some(row.get("api_key"))
}

pub async fn mark_gemini_key_used(pool: &SqlitePool, key: &str) {
    let now = now_sec();
    let _ = sqlx::query(
        "UPDATE gemini_keys SET total_uses = total_uses + 1, last_used_at = ?, status = 'healthy', status_message = '' WHERE api_key = ?",
    )
    .bind(now)
    .bind(key)
    .execute(pool)
    .await;
}

pub async fn mark_gemini_key_cooldown(pool: &SqlitePool, key: &str, cooldown_secs: f64, message: &str) {
    let now = now_sec();
    let cooldown_until = now + cooldown_secs;
    let _ = sqlx::query(
        "UPDATE gemini_keys SET status = 'quota_exceeded', status_message = ?, cooldown_until = ? WHERE api_key = ?",
    )
    .bind(message)
    .bind(cooldown_until)
    .bind(key)
    .execute(pool)
    .await;
}

// ---------------------------------------------------------------------------
// Auto-Queue configs
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn load_all_autoqueue_configs(pool: &SqlitePool) -> HashMap<String, AIAutoQueueConfig> {
    let mut map = HashMap::new();
    let rows = sqlx::query("SELECT guild_id, prompt, count, threshold, set_by_name, set_by_avatar FROM guild_autoqueue_config")
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    for r in rows {
        let gid: String = r.get("guild_id");
        let cfg = AIAutoQueueConfig {
            enabled: false, // Default to disabled on start
            prompt: r.get("prompt"),
            count: r.get("count"),
            threshold: r.get("threshold"),
            set_by_name: r.get("set_by_name"),
            set_by_avatar: r.get("set_by_avatar"),
        };
        map.insert(gid, cfg);
    }
    map
}

pub async fn save_autoqueue_config(pool: &SqlitePool, guild_id: &str, cfg: &AIAutoQueueConfig) -> Result<(), sqlx::Error> {
    let now = now_sec();
    sqlx::query(
        r#"
        INSERT INTO guild_autoqueue_config (guild_id, prompt, count, threshold, set_by_name, set_by_avatar, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(guild_id) DO UPDATE SET
            prompt = excluded.prompt,
            count = excluded.count,
            threshold = excluded.threshold,
            set_by_name = excluded.set_by_name,
            set_by_avatar = excluded.set_by_avatar,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(guild_id)
    .bind(&cfg.prompt)
    .bind(cfg.count)
    .bind(cfg.threshold)
    .bind(&cfg.set_by_name)
    .bind(&cfg.set_by_avatar)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Schedule Events
// ---------------------------------------------------------------------------

pub async fn get_schedule_events(pool: &SqlitePool, guild_id: &str) -> Vec<ScheduleEvent> {
    let rows = sqlx::query(
        r#"
        SELECT id, guild_id, title, description, category, start, duration_minutes, recurrence_json, location_json, image_url, links_json, organizer_id, organizer_name, organizer_avatar, invite_everyone, invitee_ids_json, series_responses_json, occurrence_responses_json, cancelled_occurrences_json, created_at, updated_at
        FROM guild_schedule_events
        WHERE guild_id = ?
        ORDER BY start ASC
        "#,
    )
    .bind(guild_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter().filter_map(|r| parse_schedule_row(&r)).collect()
}

pub async fn get_schedule_event(pool: &SqlitePool, event_id: &str) -> Option<ScheduleEvent> {
    let row = sqlx::query(
        r#"
        SELECT id, guild_id, title, description, category, start, duration_minutes, recurrence_json, location_json, image_url, links_json, organizer_id, organizer_name, organizer_avatar, invite_everyone, invitee_ids_json, series_responses_json, occurrence_responses_json, cancelled_occurrences_json, created_at, updated_at
        FROM guild_schedule_events
        WHERE id = ?
        "#,
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .ok()??;

    parse_schedule_row(&row)
}

fn parse_schedule_row(r: &sqlx::sqlite::SqliteRow) -> Option<ScheduleEvent> {
    let recurrence_str: Option<String> = r.get("recurrence_json");
    let recurrence = recurrence_str.and_then(|s| serde_json::from_str(&s).ok());

    let location_str: Option<String> = r.get("location_json");
    let location = location_str.and_then(|s| serde_json::from_str(&s).ok());

    let links_str: String = r.get("links_json");
    let links = serde_json::from_str(&links_str).unwrap_or_default();

    let invitee_ids_str: String = r.get("invitee_ids_json");
    let invitee_ids = serde_json::from_str(&invitee_ids_str).unwrap_or_default();

    let series_str: String = r.get("series_responses_json");
    let series_responses = serde_json::from_str(&series_str).unwrap_or_else(|_| serde_json::json!({}));

    let occ_str: String = r.get("occurrence_responses_json");
    let occurrence_responses = serde_json::from_str(&occ_str).unwrap_or_else(|_| serde_json::json!({}));

    let canc_str: String = r.get("cancelled_occurrences_json");
    let cancelled_occurrences = serde_json::from_str(&canc_str).unwrap_or_default();

    let invite_everyone_int: i64 = r.get("invite_everyone");

    Some(ScheduleEvent {
        id: r.get("id"),
        guild_id: r.get("guild_id"),
        title: r.get("title"),
        description: r.get("description"),
        category: r.get("category"),
        start: r.get("start"),
        duration_minutes: r.get("duration_minutes"),
        recurrence,
        location,
        image_url: r.get("image_url"),
        links,
        organizer_id: r.get("organizer_id"),
        organizer_name: r.get("organizer_name"),
        organizer_avatar: r.get("organizer_avatar"),
        invite_everyone: invite_everyone_int == 1,
        invitee_ids,
        series_responses,
        occurrence_responses,
        cancelled_occurrences,
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

pub async fn create_schedule_event(
    pool: &SqlitePool,
    guild_id: &str,
    data: serde_json::Value,
    organizer_id: &str,
    organizer_name: &str,
    organizer_avatar: Option<&str>,
) -> Result<ScheduleEvent, sqlx::Error> {
    let now = now_sec();
    let event_id = format!("ev-{:x}", (now * 1000.0) as u64);

    let title = data.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let description = data.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let category = data.get("category").and_then(|v| v.as_str()).unwrap_or("other").trim().to_string();
    let start = data.get("start").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let duration_minutes = data.get("durationMinutes").and_then(|v| v.as_i64()).unwrap_or(60) as i32;
    let rec_json = data.get("recurrence").filter(|v| !v.is_null()).map(|r| serde_json::to_string(r).unwrap_or_default());
    let loc_json = data.get("location").filter(|v| !v.is_null()).map(|l| serde_json::to_string(l).unwrap_or_default());
    let image_url = data.get("imageUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
    let links_json = data.get("links").filter(|v| !v.is_null()).map(|l| serde_json::to_string(l).unwrap_or_default()).unwrap_or_else(|| "[]".to_string());
    let invite_everyone = if data.get("inviteEveryone").and_then(|v| v.as_bool()).unwrap_or(true) { 1 } else { 0 };
    let invitees_json = data.get("inviteeIds").filter(|v| !v.is_null()).map(|i| serde_json::to_string(i).unwrap_or_default()).unwrap_or_else(|| "[]".to_string());

    // The organizer automatically accepts their own series
    let series_responses = serde_json::json!({
        organizer_id: "accepted"
    });
    let series_json = serde_json::to_string(&series_responses).unwrap_or_default();
    let occ_json = "{}".to_string();
    let canc_json = "[]".to_string();

    sqlx::query(
        r#"
        INSERT INTO guild_schedule_events (
            id, guild_id, title, description, category, start, duration_minutes, recurrence_json, location_json, image_url, links_json, organizer_id, organizer_name, organizer_avatar, invite_everyone, invitee_ids_json, series_responses_json, occurrence_responses_json, cancelled_occurrences_json, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&event_id)
    .bind(guild_id)
    .bind(&title)
    .bind(&description)
    .bind(&category)
    .bind(&start)
    .bind(duration_minutes)
    .bind(rec_json)
    .bind(loc_json)
    .bind(&image_url)
    .bind(links_json)
    .bind(organizer_id)
    .bind(organizer_name)
    .bind(organizer_avatar)
    .bind(invite_everyone)
    .bind(invitees_json)
    .bind(series_json)
    .bind(occ_json)
    .bind(canc_json)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    let event = get_schedule_event(pool, &event_id)
        .await
        .ok_or(sqlx::Error::RowNotFound)?;

    Ok(event)
}

pub async fn update_schedule_event(
    pool: &SqlitePool,
    event_id: &str,
    payload: &serde_json::Value,
    user_id: &str,
    is_admin: bool,
) -> Result<Option<ScheduleEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let existing = match get_schedule_event(pool, event_id).await {
        Some(e) => e,
        None => return Ok(None),
    };

    if existing.organizer_id != user_id && !is_admin {
        return Err("Permission denied: only event organizer or admin can edit this event".into());
    }

    let now = now_sec();
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or(&existing.title);
    let description = payload.get("description").and_then(|v| v.as_str()).unwrap_or(&existing.description);
    let category = payload.get("category").and_then(|v| v.as_str()).or(existing.category.as_deref());
    let start = payload.get("start").and_then(|v| v.as_str()).unwrap_or(&existing.start);
    let duration_minutes = payload.get("durationMinutes").and_then(|v| v.as_i64()).map(|n| n as i32).unwrap_or(existing.duration_minutes);

    let recurrence_json = payload.get("recurrence").map(|v| serde_json::to_string(v).unwrap_or_default())
        .or_else(|| existing.recurrence.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default()));

    let location_json = payload.get("location").map(|v| serde_json::to_string(v).unwrap_or_default())
        .or_else(|| existing.location.as_ref().map(|l| serde_json::to_string(l).unwrap_or_default()));

    let image_url = payload.get("imageUrl").and_then(|v| v.as_str()).map(|s| s.to_string()).or(existing.image_url);

    sqlx::query(
        r#"
        UPDATE guild_schedule_events SET
            title = ?, description = ?, category = ?, start = ?, duration_minutes = ?, recurrence_json = ?, location_json = ?, image_url = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(title)
    .bind(description)
    .bind(category)
    .bind(start)
    .bind(duration_minutes)
    .bind(recurrence_json)
    .bind(location_json)
    .bind(image_url)
    .bind(now)
    .bind(event_id)
    .execute(pool)
    .await?;

    Ok(get_schedule_event(pool, event_id).await)
}

pub async fn delete_schedule_event(
    pool: &SqlitePool,
    event_id: &str,
    user_id: &str,
    is_admin: bool,
    occurrence: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let existing = match get_schedule_event(pool, event_id).await {
        Some(e) => e,
        None => return Ok(false),
    };

    if existing.organizer_id != user_id && !is_admin {
        return Err("Permission denied: only event organizer or admin can delete this event".into());
    }

    if let Some(occ) = occurrence {
        // Cancel single occurrence
        let mut canc = existing.cancelled_occurrences;
        if !canc.contains(&occ.to_string()) {
            canc.push(occ.to_string());
        }
        let canc_json = serde_json::to_string(&canc).unwrap_or_default();
        sqlx::query("UPDATE guild_schedule_events SET cancelled_occurrences_json = ? WHERE id = ?")
            .bind(canc_json)
            .bind(event_id)
            .execute(pool)
            .await?;
        return Ok(true);
    }

    let res = sqlx::query("DELETE FROM guild_schedule_events WHERE id = ?")
        .bind(event_id)
        .execute(pool)
        .await?;

    let _ = sqlx::query("DELETE FROM guild_schedule_reminders WHERE event_id = ?")
        .bind(event_id)
        .execute(pool)
        .await;

    Ok(res.rows_affected() > 0)
}

pub async fn respond_schedule_event(
    pool: &SqlitePool,
    event_id: &str,
    user_id: &str,
    key: &str,
    status: &str,
    scope: &str,
) -> Option<ScheduleEvent> {
    let existing = get_schedule_event(pool, event_id).await?;

    if scope == "series" {
        let mut series = existing.series_responses;
        if let Some(obj) = series.as_object_mut() {
            obj.insert(user_id.to_string(), serde_json::Value::String(status.to_string()));
        }
        let series_json = serde_json::to_string(&series).unwrap_or_default();
        sqlx::query("UPDATE guild_schedule_events SET series_responses_json = ? WHERE id = ?")
            .bind(series_json)
            .bind(event_id)
            .execute(pool)
            .await
            .ok()?;
    } else {
        let mut occ = existing.occurrence_responses;
        if let Some(obj) = occ.as_object_mut() {
            let occ_map = obj.entry(key.to_string()).or_insert_with(|| serde_json::json!({}));
            if let Some(map) = occ_map.as_object_mut() {
                map.insert(user_id.to_string(), serde_json::Value::String(status.to_string()));
            }
        }
        let occ_json = serde_json::to_string(&occ).unwrap_or_default();
        sqlx::query("UPDATE guild_schedule_events SET occurrence_responses_json = ? WHERE id = ?")
            .bind(occ_json)
            .bind(event_id)
            .execute(pool)
            .await
            .ok()?;
    }

    get_schedule_event(pool, event_id).await
}

pub async fn get_all_active_schedule_events(pool: &SqlitePool) -> Vec<ScheduleEvent> {
    let rows = sqlx::query(
        r#"
        SELECT id, guild_id, title, description, category, start, duration_minutes, recurrence_json, location_json, image_url, links_json, organizer_id, organizer_name, organizer_avatar, invite_everyone, invitee_ids_json, series_responses_json, occurrence_responses_json, cancelled_occurrences_json, created_at, updated_at
        FROM guild_schedule_events
        ORDER BY start ASC
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.iter().filter_map(parse_schedule_row).collect()
}

pub async fn is_occurrence_reminded(pool: &SqlitePool, event_id: &str, occurrence_key: &str, reminder_type: &str) -> bool {
    sqlx::query("SELECT 1 FROM guild_schedule_reminders WHERE event_id = ? AND occurrence_key = ? AND reminder_type = ?")
        .bind(event_id)
        .bind(occurrence_key)
        .bind(reminder_type)
        .fetch_optional(pool)
        .await
        .unwrap_or(None)
        .is_some()
}

pub async fn mark_occurrence_reminded(pool: &SqlitePool, event_id: &str, occurrence_key: &str, reminder_type: &str) {
    let now = now_sec();
    let _ = sqlx::query("INSERT OR REPLACE INTO guild_schedule_reminders (event_id, occurrence_key, reminder_type, reminded_at) VALUES (?, ?, ?, ?)")
        .bind(event_id)
        .bind(occurrence_key)
        .bind(reminder_type)
        .bind(now)
        .execute(pool)
        .await;
}

pub async fn clear_event_reminders(pool: &SqlitePool, event_id: &str) {
    let _ = sqlx::query("DELETE FROM guild_schedule_reminders WHERE event_id = ?")
        .bind(event_id)
        .execute(pool)
        .await;
}

