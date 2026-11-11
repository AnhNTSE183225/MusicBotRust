use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub url: String,
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: f64,
    #[serde(default)]
    pub requester_id: Option<String>,
    #[serde(default)]
    pub requester_name: Option<String>,
    #[serde(default)]
    pub requester_avatar: Option<String>,
    #[serde(default)]
    pub requester_handle: Option<String>,
    #[serde(default)]
    pub queue_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackStatus {
    Disconnected,
    Idle,
    Loading,
    Playing,
    Paused,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        PlaybackStatus::Disconnected
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStateResponse {
    pub status: PlaybackStatus,
    pub queue: Vec<Song>,
    pub current: Option<Song>,
    pub queue_index: i32,
    pub volume: i32,
    #[serde(rename = "loop")]
    pub loop_enabled: bool,
    pub is_playing: bool,
    pub is_paused: bool,
    pub bot_connected: bool,
    pub position: f64,
    pub skip_votes: u32,
    pub skip_votes_required: u32,
    pub pause_votes: u32,
    pub pause_votes_required: u32,
    pub resume_votes: u32,
    pub resume_votes_required: u32,
    pub stop_votes: u32,
    pub stop_votes_required: u32,
    #[serde(default)]
    pub ai_auto_enabled: bool,
    #[serde(default)]
    pub ai_prompt: String,
    #[serde(default)]
    pub ai_set_by_name: Option<String>,
    #[serde(default)]
    pub ai_set_by_avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSongRequest {
    pub url: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIBatchConfirmRequest {
    pub songs: Vec<AddSongRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRequest {
    pub volume: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRequest {
    pub index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistPreviewRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrackItem {
    pub index: i32,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub url: String,
    pub thumbnail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistPreviewResponse {
    pub title: String,
    pub uploader: String,
    pub thumbnail: String,
    pub total_tracks: usize,
    pub tracks: Vec<PlaylistTrackItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddPlaylistRequest {
    pub url: Option<String>,
    pub title: Option<String>,
    pub start_index: Option<i32>,
    pub end_index: Option<i32>,
    pub tracks: Option<Vec<AddSongRequest>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContributeKeyRequest {
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AISearchRequest {
    pub prompt: String,
    #[serde(default = "default_ai_count")]
    pub count: i32,
    #[serde(default = "default_ai_threshold")]
    pub threshold: i32,
}

fn default_ai_count() -> i32 { 5 }
fn default_ai_threshold() -> i32 { 2 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIAutoQueueConfig {
    pub enabled: bool,
    pub prompt: String,
    #[serde(default = "default_ai_count")]
    pub count: i32,
    #[serde(default = "default_ai_threshold")]
    pub threshold: i32,
    pub set_by_name: Option<String>,
    pub set_by_avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiKeyInfo {
    pub id: i64,
    pub masked_key: String,
    pub added_by_user_id: String,
    pub added_by_username: String,
    pub added_by_avatar: Option<String>,
    pub added_at: f64,
    pub is_active: bool,
    pub status: String,
    pub status_message: String,
    pub cooldown_remaining_seconds: f64,
    pub total_uses: i64,
    pub last_used_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct GeminiKeysResponse {
    pub keys: Vec<GeminiKeyInfo>,
    pub total_active: usize,
    pub total_keys: usize,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildSession {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub bot_connected: bool,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMember {
    pub id: String,
    pub name: String,
    pub avatar: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerChannel {
    pub id: String,
    pub name: String,
    pub r#type: String, // "voice" | "stage"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEventLocation {
    pub kind: String, // 'discord' | 'address' | 'link'
    #[serde(rename = "channelId")]
    pub channel_id: Option<String>,
    #[serde(rename = "channelName")]
    pub channel_name: Option<String>,
    pub address: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEventLink {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEventRecurrence {
    pub freq: String, // 'daily' | 'weekly' | 'monthly'
    pub interval: i32,
    pub weekdays: Option<Vec<i32>>,
    pub until: Option<String>,
    pub count: Option<i32>,
}

fn default_duration() -> i32 {
    60
}

fn default_true() -> bool {
    true
}

fn default_empty_json() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEvent {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "guildId")]
    pub guild_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub category: Option<String>,
    #[serde(default)]
    pub start: String,
    #[serde(default = "default_duration", rename = "durationMinutes")]
    pub duration_minutes: i32,
    pub recurrence: Option<ScheduleEventRecurrence>,
    pub location: Option<ScheduleEventLocation>,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
    #[serde(default)]
    pub links: Vec<ScheduleEventLink>,
    #[serde(default, rename = "organizerId")]
    pub organizer_id: String,
    #[serde(rename = "organizerName")]
    pub organizer_name: Option<String>,
    #[serde(rename = "organizerAvatar")]
    pub organizer_avatar: Option<String>,
    #[serde(default = "default_true", rename = "inviteEveryone")]
    pub invite_everyone: bool,
    #[serde(default, rename = "inviteeIds")]
    pub invitee_ids: Vec<String>,
    #[serde(default = "default_empty_json", rename = "seriesResponses")]
    pub series_responses: serde_json::Value,
    #[serde(default = "default_empty_json", rename = "occurrenceResponses")]
    pub occurrence_responses: serde_json::Value,
    #[serde(default, rename = "cancelledOccurrences")]
    pub cancelled_occurrences: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<f64>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ScheduleDataResponse {
    pub events: Vec<ScheduleEvent>,
    pub members: Vec<ServerMember>,
    pub channels: Vec<ServerChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub discriminator: Option<String>,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordGuild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub owner: Option<bool>,
    pub permissions: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSession {
    pub session_id: String,
    pub user: DiscordUser,
    pub guilds: Vec<DiscordGuild>,
    pub access_token: String,
    pub created_at: f64,
    pub expires_at: f64,
}
