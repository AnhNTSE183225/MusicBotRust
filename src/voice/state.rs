use std::collections::HashSet;
use songbird::tracks::TrackHandle;
use tracing::info;
use crate::models::{PlaybackStatus, QueueStateResponse, Song};

#[derive(Clone)]
#[allow(dead_code)]
pub struct GuildMusicState {
    pub guild_id: u64,
    pub status: PlaybackStatus,
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
            status: PlaybackStatus::Disconnected,
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

    pub fn transition_to(&mut self, next: PlaybackStatus, reason: impl std::fmt::Display) {
        if self.status != next {
            info!(
                "[Voice State Machine] Guild {}: {:?} -> {:?} ({})",
                self.guild_id, self.status, next, reason
            );
            self.status = next;
            self.is_playing = next == PlaybackStatus::Playing;
            self.is_paused = next == PlaybackStatus::Paused;
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
        let mut effective_status = if !bot_connected {
            PlaybackStatus::Disconnected
        } else {
            self.status
        };

        // Query active Songbird track handle if connected
        if bot_connected {
            if let Some(handle) = &self.track_handle {
                if let Ok(info) = handle.get_info().await {
                    position = info.position.as_secs_f64();
                    match info.playing {
                        songbird::tracks::PlayMode::Play => {
                            effective_status = PlaybackStatus::Playing;
                        }
                        songbird::tracks::PlayMode::Pause => {
                            effective_status = PlaybackStatus::Paused;
                        }
                        songbird::tracks::PlayMode::Stop | songbird::tracks::PlayMode::End => {
                            if effective_status != PlaybackStatus::Loading {
                                effective_status = PlaybackStatus::Idle;
                            }
                        }
                        _ => {}
                    }
                } else if effective_status != PlaybackStatus::Loading {
                    effective_status = PlaybackStatus::Idle;
                }
            } else if effective_status == PlaybackStatus::Playing || effective_status == PlaybackStatus::Paused {
                effective_status = PlaybackStatus::Idle;
            }
        }

        let is_playing = effective_status == PlaybackStatus::Playing;
        let is_paused = effective_status == PlaybackStatus::Paused;

        let mut queue = self.queue.clone();
        for song in &mut queue {
            Self::normalize_song(song);
        }

        let mut current = if is_playing || is_paused || effective_status == PlaybackStatus::Loading {
            self.current_song.clone()
        } else {
            None
        };
        if let Some(song) = &mut current {
            Self::normalize_song(song);
        }

        let has_current = current.is_some();
        QueueStateResponse {
            status: effective_status,
            queue,
            current,
            queue_index: if has_current { self.queue_index } else { -1 },
            volume: (self.volume * 100.0).round() as i32,
            loop_enabled: self.loop_enabled,
            is_playing,
            is_paused,
            bot_connected,
            position: if is_playing || is_paused { position } else { 0.0 },
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
