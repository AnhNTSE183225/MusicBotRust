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
    pub pause_votes: HashSet<u64>,
    pub resume_votes: HashSet<u64>,
    pub stop_votes: HashSet<u64>,
    pub track_handle: Option<TrackHandle>,
    pub ai_auto_enabled: bool,
    pub ai_prompt: String,
    pub ai_set_by_name: Option<String>,
    pub ai_set_by_avatar: Option<String>,
    pub playback_error: Option<String>,
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
            pause_votes: HashSet::new(),
            resume_votes: HashSet::new(),
            stop_votes: HashSet::new(),
            track_handle: None,
            ai_auto_enabled: false,
            ai_prompt: String::new(),
            ai_set_by_name: None,
            ai_set_by_avatar: None,
            playback_error: None,
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

    pub fn should_auto_start_on_enqueue(&self) -> bool {
        matches!(self.status, PlaybackStatus::Idle | PlaybackStatus::Disconnected)
    }

    pub fn advance_next_track(&mut self) -> Option<Song> {
        let next_idx = self.queue_index + 1;
        if next_idx >= 0 && (next_idx as usize) < self.queue.len() {
            self.queue_index = next_idx;
            let s = self.queue[next_idx as usize].clone();
            self.current_song = Some(s.clone());
            self.transition_to(PlaybackStatus::Loading, format!("preparing track '{}'", s.title));
            Some(s)
        } else if self.loop_enabled && !self.queue.is_empty() {
            self.queue_index = 0;
            let s = self.queue[0].clone();
            self.current_song = Some(s.clone());
            self.transition_to(
                PlaybackStatus::Loading,
                format!("looping playlist, preparing track '{}'", s.title),
            );
            Some(s)
        } else {
            self.transition_to(PlaybackStatus::Idle, "queue completed (no more tracks)");
            self.track_handle = None;
            self.current_song = None;
            // Preserve cursor at last finished track (len - 1) so appending a track continues at queue[len].
            self.queue_index = if self.queue.is_empty() {
                -1
            } else {
                (self.queue.len() as i32) - 1
            };
            None
        }
    }

    pub fn clear_queue_preserving_active(&mut self) {
        if matches!(self.status, PlaybackStatus::Idle | PlaybackStatus::Disconnected) {
            self.queue.clear();
            self.queue_index = -1;
            self.playback_error = None;
        } else {
            let cur_idx = self.queue_index as usize;
            if cur_idx < self.queue.len() {
                self.queue.truncate(cur_idx + 1);
            } else {
                self.queue.clear();
                self.queue_index = -1;
            }
        }
    }

    pub fn mark_current_track_failed(&mut self, song_title: &str, reason: &str) {
        let friendly = format!("'{}' is unavailable or having issues: {}", song_title, reason);
        if self.queue_index >= 0 {
            if let Some(track) = self.queue.get_mut(self.queue_index as usize) {
                track.error = Some(reason.to_string());
            }
        }
        self.playback_error = Some(friendly);
        self.track_handle = None;
        self.current_song = None;
        self.transition_to(
            PlaybackStatus::Idle,
            format!("audio stream failed for '{}'", song_title),
        );
    }

    pub fn clear_track_error(&mut self) {
        self.playback_error = None;
        if self.queue_index >= 0 {
            if let Some(track) = self.queue.get_mut(self.queue_index as usize) {
                track.error = None;
            }
        }
    }

    pub fn remove_queue_item(&mut self, one_based_index: usize) {
        if one_based_index > 0 && one_based_index <= self.queue.len() {
            let zero_idx = one_based_index - 1;
            self.queue.remove(zero_idx);
            let is_idle = matches!(self.status, PlaybackStatus::Idle | PlaybackStatus::Disconnected);
            if (zero_idx as i32) < self.queue_index
                || (is_idle && (zero_idx as i32) == self.queue_index)
            {
                self.queue_index -= 1;
            }
            if self.queue.is_empty() {
                self.queue_index = -1;
            }
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

    pub async fn to_response(&self, bot_connected: bool, required_votes: u32) -> QueueStateResponse {
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
            skip_votes_required: required_votes,
            pause_votes: self.pause_votes.len() as u32,
            pause_votes_required: required_votes,
            resume_votes: self.resume_votes.len() as u32,
            resume_votes_required: required_votes,
            stop_votes: self.stop_votes.len() as u32,
            stop_votes_required: required_votes,
            ai_auto_enabled: self.ai_auto_enabled,
            ai_prompt: self.ai_prompt.clone(),
            ai_set_by_name: self.ai_set_by_name.clone(),
            ai_set_by_avatar: self.ai_set_by_avatar.clone(),
            playback_error: self.playback_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_song(title: &str) -> Song {
        Song {
            url: format!("https://www.youtube.com/watch?v={}", title),
            title: title.to_string(),
            artist: None,
            thumbnail: None,
            duration: 100.0,
            requester_id: None,
            requester_name: None,
            requester_avatar: None,
            requester_handle: None,
            queue_id: None,
            error: None,
            stream_url: None,
            stream_user_agent: None,
        }
    }

    #[test]
    fn adding_second_song_after_first_finishes_plays_second_song_not_first() {
        let mut state = GuildMusicState::new(1, 0.75);
        state.transition_to(PlaybackStatus::Idle, "joined");

        // Enqueue song 1 and advance
        state.queue.push(sample_song("XUrmskUYoas"));
        assert!(state.should_auto_start_on_enqueue());
        let first = state.advance_next_track().expect("should select song 1");
        assert_eq!(first.title, "XUrmskUYoas");
        assert_eq!(state.queue_index, 0);
        assert_eq!(state.status, PlaybackStatus::Loading);

        // Simulate song 1 playing, then finishing
        state.transition_to(PlaybackStatus::Playing, "started");
        let after_first = state.advance_next_track();
        assert!(after_first.is_none());
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert!(state.current_song.is_none());
        assert_eq!(state.queue_index, 0);

        // User adds song 2 while Idle -> must continue to song 2 (index 1), not replay song 1 (index 0)
        state.queue.push(sample_song("WWEs82u37Mw"));
        assert!(state.should_auto_start_on_enqueue());
        let second = state.advance_next_track().expect("should select song 2");
        assert_eq!(second.title, "WWEs82u37Mw");
        assert_eq!(state.queue_index, 1);
        assert_eq!(state.status, PlaybackStatus::Loading);
    }

    #[test]
    fn enqueue_while_loading_or_paused_does_not_interrupt_current_track() {
        let mut state = GuildMusicState::new(1, 0.75);
        state.transition_to(PlaybackStatus::Loading, "buffering song 1");
        assert!(!state.should_auto_start_on_enqueue());

        state.transition_to(PlaybackStatus::Paused, "paused song 1");
        assert!(!state.should_auto_start_on_enqueue());
    }

    #[test]
    fn clear_and_remove_keep_queue_index_consistent_when_idle() {
        let mut state = GuildMusicState::new(1, 0.75);
        state.queue.push(sample_song("song1"));
        state.advance_next_track();
        state.transition_to(PlaybackStatus::Playing, "playing");
        state.advance_next_track(); // finishes -> Idle, queue_index = 0

        state.remove_queue_item(1);
        assert!(state.queue.is_empty());
        assert_eq!(state.queue_index, -1);

        state.queue.push(sample_song("song1"));
        state.advance_next_track();
        state.transition_to(PlaybackStatus::Playing, "playing");
        state.advance_next_track(); // Idle, queue_index = 0
        state.clear_queue_preserving_active();
        assert!(state.queue.is_empty());
        assert_eq!(state.queue_index, -1);
    }

    #[test]
    fn mark_current_track_failed_records_error_and_marks_queue_entry() {
        let mut state = GuildMusicState::new(1, 0.75);
        state.queue.push(sample_song("broken_vid"));
        state.advance_next_track();
        state.mark_current_track_failed("broken_vid", "Unavailable (HTTP 403)");

        assert_eq!(state.status, PlaybackStatus::Idle);
        assert!(state.current_song.is_none());
        assert_eq!(
            state.queue[0].error.as_deref(),
            Some("Unavailable (HTTP 403)")
        );
        assert!(state
            .playback_error
            .as_deref()
            .unwrap()
            .contains("broken_vid"));

        state.clear_track_error();
        assert!(state.playback_error.is_none());
        assert!(state.queue[0].error.is_none());
    }
}
