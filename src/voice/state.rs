use std::collections::HashSet;
use songbird::tracks::TrackHandle;
use crate::models::{QueueStateResponse, Song};

#[derive(Clone)]
#[allow(dead_code)]
pub struct GuildMusicState {
    pub guild_id: u64,
    pub channel_id: Option<u64>,
    pub text_channel_id: Option<u64>,
    pub queue: Vec<Song>,
    pub current_song: Option<Song>,
    pub queue_index: i32,
    pub volume: f32,
    pub loop_enabled: bool,
    pub is_playing: bool,
    pub is_paused: bool,
    pub skip_votes: HashSet<u64>,
    pub track_handle: Option<TrackHandle>,
    pub ai_auto_enabled: bool,
    pub ai_prompt: String,
    pub ai_set_by_name: Option<String>,
    pub ai_set_by_avatar: Option<String>,
}

impl GuildMusicState {
    pub fn new(guild_id: u64, default_volume: f32) -> Self {
        Self {
            guild_id,
            channel_id: None,
            text_channel_id: None,
            queue: Vec::new(),
            current_song: None,
            queue_index: -1,
            volume: default_volume,
            loop_enabled: false,
            is_playing: false,
            is_paused: false,
            skip_votes: HashSet::new(),
            track_handle: None,
            ai_auto_enabled: false,
            ai_prompt: String::new(),
            ai_set_by_name: None,
            ai_set_by_avatar: None,
        }
    }

    fn normalize_song(song: &mut Song) {
        if song.requester_handle.is_none() {
            song.requester_handle = song.requester_name.clone();
        }
        if let (Some(av), Some(uid)) = (&song.requester_avatar, &song.requester_id) {
            if !av.starts_with("http") && !av.starts_with('/') {
                song.requester_avatar = Some(format!(
                    "https://cdn.discordapp.com/avatars/{}/{}.png?size=64",
                    uid, av
                ));
            }
        }
    }

    pub async fn to_response(&self, bot_connected: bool) -> QueueStateResponse {
        let mut position = 0.0;
        let mut is_paused = self.is_paused;

        if let Some(handle) = &self.track_handle {
            if let Ok(info) = handle.get_info().await {
                position = info.position.as_secs_f64();
                is_paused = info.playing == songbird::tracks::PlayMode::Pause;
            }
        }

        let mut queue = self.queue.clone();
        for song in &mut queue {
            Self::normalize_song(song);
        }

        let mut current = self.current_song.clone();
        if let Some(song) = &mut current {
            Self::normalize_song(song);
        }

        QueueStateResponse {
            queue,
            current,
            queue_index: self.queue_index,
            volume: (self.volume * 100.0).round() as i32,
            loop_enabled: self.loop_enabled,
            is_playing: self.is_playing,
            is_paused,
            bot_connected,
            position,
            skip_votes: self.skip_votes.len() as u32,
            skip_votes_required: 1,
            pause_votes: 0,
            pause_votes_required: 1,
            resume_votes: 0,
            resume_votes_required: 1,
            stop_votes: 0,
            stop_votes_required: 1,
            ai_auto_enabled: self.ai_auto_enabled,
            ai_prompt: self.ai_prompt.clone(),
            ai_set_by_name: self.ai_set_by_name.clone(),
            ai_set_by_avatar: self.ai_set_by_avatar.clone(),
        }
    }
}
