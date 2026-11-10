use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use async_trait::async_trait;
use reqwest::Client;
use serenity::all::{ChannelId, GuildId};
use songbird::events::{Event, EventContext, EventHandler, TrackEvent};
use songbird::Songbird;
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use crate::models::{QueueStateResponse, Song};
use crate::voice::state::GuildMusicState;

pub struct VoiceManager {
    songbird: Arc<Songbird>,
    states: Arc<RwLock<HashMap<u64, Arc<RwLock<GuildMusicState>>>>>,
    http_client: Client,
    db_pool: SqlitePool,
    default_volume: f32,
}

impl VoiceManager {
    pub fn new(
        songbird: Arc<Songbird>,
        db_pool: SqlitePool,
        default_volume: f32,
    ) -> Self {
        Self {
            songbird,
            states: Arc::new(RwLock::new(HashMap::new())),
            http_client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
                .build()
                .unwrap_or_default(),
            db_pool,
            default_volume,
        }
    }

    pub async fn get_or_create_state(&self, guild_id: u64) -> Arc<RwLock<GuildMusicState>> {
        let mut states = self.states.write().await;
        states
            .entry(guild_id)
            .or_insert_with(|| Arc::new(RwLock::new(GuildMusicState::new(guild_id, self.default_volume))))
            .clone()
    }

    pub async fn join_channel(
        self: &Arc<Self>,
        guild_id: u64,
        channel_id: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Voice] Joining guild {} channel {}", guild_id, channel_id);
        match self.songbird.join(GuildId::new(guild_id), ChannelId::new(channel_id)).await {
            Ok(_) => {
                info!("[Voice] Successfully joined guild {} channel {}", guild_id, channel_id);
            }
            Err(e) => {
                error!("[Voice] Join channel failed for guild {} channel {}: {}", guild_id, channel_id, e);
                return Err(Box::new(e));
            }
        }

        let state_arc = self.get_or_create_state(guild_id).await;
        {
            let mut state = state_arc.write().await;
            state.channel_id = Some(channel_id);
        }

        let should_start = {
            let state = state_arc.read().await;
            !state.is_playing && state.track_handle.is_none() && (state.current_song.is_some() || !state.queue.is_empty())
        };

        if should_start {
            info!("[Voice] Pending queue detected, triggering playback upon channel join");
            self.play_current(guild_id).await?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn leave_channel(
        &self,
        guild_id: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Voice] Leaving guild {}", guild_id);
        self.stop(guild_id).await?;
        let _ = self.songbird.leave(GuildId::new(guild_id)).await;
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        state.channel_id = None;
        state.is_playing = false;
        state.track_handle = None;
        Ok(())
    }

    pub async fn is_connected(&self, guild_id: u64) -> bool {
        if let Some(call_lock) = self.songbird.get(GuildId::new(guild_id)) {
            let call = call_lock.lock().await;
            call.current_channel().is_some()
        } else {
            false
        }
    }

    pub async fn enqueue_song(
        self: &Arc<Self>,
        guild_id: u64,
        song: Song,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let should_start = {
            let mut state = state_arc.write().await;
            state.queue.push(song);
            !state.is_playing || state.track_handle.is_none()
        };

        if should_start {
            self.play_next(guild_id).await?;
        }
        Ok(())
    }

    pub async fn enqueue_batch(
        self: &Arc<Self>,
        guild_id: u64,
        songs: Vec<Song>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let count = songs.len();
        let state_arc = self.get_or_create_state(guild_id).await;
        let should_start = {
            let mut state = state_arc.write().await;
            for song in songs {
                state.queue.push(song);
            }
            !state.is_playing || state.track_handle.is_none()
        };

        if should_start {
            let _ = self.play_next(guild_id).await;
        }
        Ok(count)
    }

    pub async fn play_current(
        self: &Arc<Self>,
        guild_id: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let song = {
            let state = state_arc.read().await;
            state.current_song.clone()
        };

        match song {
            Some(s) => self.play_song_internal(guild_id, s).await,
            None => self.play_next(guild_id).await,
        }
    }

    pub async fn play_next(
        self: &Arc<Self>,
        guild_id: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.is_connected(guild_id).await {
            warn!("[Voice] Not connected to voice channel in guild {}", guild_id);
            let state_arc = self.get_or_create_state(guild_id).await;
            let mut state = state_arc.write().await;
            state.is_playing = false;
            state.track_handle = None;
            return Ok(());
        }

        let state_arc = self.get_or_create_state(guild_id).await;
        let next_song = {
            let mut state = state_arc.write().await;
            if state.loop_enabled && state.current_song.is_some() {
                state.current_song.clone()
            } else {
                let next_idx = state.queue_index + 1;
                if next_idx >= 0 && (next_idx as usize) < state.queue.len() {
                    state.queue_index = next_idx;
                    let s = state.queue[next_idx as usize].clone();
                    state.current_song = Some(s.clone());
                    Some(s)
                } else {
                    state.is_playing = false;
                    state.track_handle = None;
                    None
                }
            }
        };

        let song = match next_song {
            Some(s) => s,
            None => {
                info!("[Voice] Queue ended for guild {}", guild_id);
                return Ok(());
            }
        };

        self.play_song_internal(guild_id, song).await
    }

    pub async fn play_song_internal(
        self: &Arc<Self>,
        guild_id: u64,
        song: Song,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let handler_lock = match self.songbird.get(GuildId::new(guild_id)) {
            Some(h) => h,
            None => {
                warn!("[Voice] Not connected to voice channel in guild {}", guild_id);
                let state_arc = self.get_or_create_state(guild_id).await;
                let mut state = state_arc.write().await;
                state.is_playing = false;
                state.track_handle = None;
                return Ok(());
            }
        };

        info!("[Voice] Playing track: '{}' ({})", song.title, song.url);
        let mut ytdl = std::process::Command::new("yt-dlp")
            .args([
                "--extractor-args",
                "youtube:player_client=android,ios,mweb,web",
                "-o",
                "-",
                "-f",
                "ba/b",
                "--no-warnings",
                &song.url,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let ytdl_stdout = ytdl.stdout.take().ok_or("Failed to capture yt-dlp stdout")?;

        let ffmpeg = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "warning",
                "-i",
                "pipe:0",
                "-f",
                "f32le",
                "-ar",
                "48000",
                "-ac",
                "2",
                "pipe:1",
            ])
            .stdin(ytdl_stdout)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let container = songbird::input::ChildContainer::from(vec![ytdl, ffmpeg]);
        let raw_adapter = songbird::input::RawAdapter::new(
            symphonia::core::io::ReadOnlySource::new(container),
            48000,
            2,
        );
        let input: songbird::input::Input = raw_adapter.into();

        let handle = {
            let mut handler = handler_lock.lock().await;
            handler.play_input(input)
        };

        // Configure volume
        let state_arc = self.get_or_create_state(guild_id).await;
        {
            let state = state_arc.read().await;
            let _ = handle.set_volume(state.volume);
        }

        // Register track end event
        let notifier = TrackEndNotifier {
            manager: self.clone(),
            guild_id,
        };

        let _ = handle.add_event(Event::Track(TrackEvent::End), notifier);

        {
            let mut state = state_arc.write().await;
            state.track_handle = Some(handle);
            state.is_playing = true;
            state.is_paused = false;
            state.skip_votes.clear();
        }

        // Check if autoqueue needs to trigger
        self.check_autoqueue(guild_id).await;

        Ok(())
    }

    pub async fn check_autoqueue(self: &Arc<Self>, guild_id: u64) {
        let (auto_enabled, prompt, threshold, count, songs_left) = {
            let state_arc = self.get_or_create_state(guild_id).await;
            let state = state_arc.read().await;
            let left = if state.queue_index >= 0 && (state.queue_index as usize) < state.queue.len() {
                state.queue.len() - 1 - (state.queue_index as usize)
            } else {
                state.queue.len()
            };
            (
                state.ai_auto_enabled,
                state.ai_prompt.clone(),
                2usize,
                5usize,
                left,
            )
        };

        if auto_enabled && !prompt.is_empty() && songs_left < threshold {
            info!(
                "[AI AutoQueue] Songs left ({}) < threshold ({}). Auto-filling for guild {}",
                songs_left, threshold, guild_id
            );

            let mgr = self.clone();
            tokio::spawn(async move {
                let pool = mgr.db_pool.clone();
                let client = mgr.http_client.clone();
                if let Ok(queries) = crate::services::ai::generate_search_queries(&client, &pool, &prompt, count * 2).await {
                    let songs = crate::services::ai::search_songs(&queries, count, &HashSet::new()).await;
                    if !songs.is_empty() {
                        let state_arc = mgr.get_or_create_state(guild_id).await;
                        let mut state = state_arc.write().await;
                        for song in songs {
                            state.queue.push(song);
                        }
                    }
                }
            });
        }
    }

    pub async fn skip(self: &Arc<Self>, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Voice] Skip invoked for guild {}", guild_id);
        let state_arc = self.get_or_create_state(guild_id).await;
        let handle = {
            let state = state_arc.read().await;
            state.track_handle.clone()
        };

        if let Some(h) = handle {
            let _ = h.stop();
        } else {
            self.play_next(guild_id).await?;
        }
        Ok(())
    }

    pub async fn skipto(self: &Arc<Self>, guild_id: u64, index: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Voice] Skipto {} for guild {}", index, guild_id);
        let state_arc = self.get_or_create_state(guild_id).await;
        {
            let mut state = state_arc.write().await;
            if index > 0 && index <= state.queue.len() {
                // Set to index - 2 so play_next increments to index - 1
                state.queue_index = (index as i32) - 2;
            }
        }
        self.skip(guild_id).await?;
        Ok(())
    }

    pub async fn pause(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if let Some(h) = &state.track_handle {
            let _ = h.pause();
        }
        state.is_paused = true;
        Ok(())
    }

    pub async fn resume(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if let Some(h) = &state.track_handle {
            let _ = h.play();
        }
        state.is_paused = false;
        Ok(())
    }

    pub async fn stop(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if let Some(h) = &state.track_handle {
            let _ = h.stop();
        }
        state.track_handle = None;
        state.is_playing = false;
        state.is_paused = false;
        state.queue.clear();
        state.current_song = None;
        state.queue_index = -1;
        state.skip_votes.clear();
        Ok(())
    }

    pub async fn clear(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        let cur_idx = state.queue_index as usize;
        if cur_idx < state.queue.len() {
            state.queue.truncate(cur_idx + 1);
        } else {
            state.queue.clear();
        }
        Ok(())
    }

    pub async fn remove(&self, guild_id: u64, index: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if index > 0 && index <= state.queue.len() {
            let zero_idx = index - 1;
            state.queue.remove(zero_idx);
            if (zero_idx as i32) < state.queue_index {
                state.queue_index -= 1;
            }
        }
        Ok(())
    }

    pub async fn set_volume(&self, guild_id: u64, volume: i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let vol_f32 = (volume.clamp(0, 200) as f32) / 100.0;
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        state.volume = vol_f32;
        if let Some(h) = &state.track_handle {
            let _ = h.set_volume(vol_f32);
        }
        Ok(())
    }

    pub async fn set_loop(&self, guild_id: u64, loop_enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        state.loop_enabled = loop_enabled;
        Ok(())
    }

    pub async fn get_queue_state(&self, guild_id: u64) -> QueueStateResponse {
        let state_arc = self.get_or_create_state(guild_id).await;
        let state = state_arc.read().await;
        let connected = self.is_connected(guild_id).await;
        state.to_response(connected).await
    }
}

struct TrackEndNotifier {
    manager: Arc<VoiceManager>,
    guild_id: u64,
}

#[async_trait]
impl EventHandler for TrackEndNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        if let EventContext::Track(track_list) = ctx {
            for (state, _) in *track_list {
                info!(
                    "[Voice] Track ended for guild {}: playing={:?}, position={:?}",
                    self.guild_id, state.playing, state.position
                );
            }
            let mgr = self.manager.clone();
            let gid = self.guild_id;
            tokio::spawn(async move {
                let _ = mgr.play_next(gid).await;
            });
        }
        None
    }
}
