use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use async_trait::async_trait;
use reqwest::Client;
use serenity::all::{Cache, ChannelId, GuildId, UserId};
use songbird::events::{Event, EventContext, EventHandler, TrackEvent};
use songbird::Songbird;
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use crate::models::{PlaybackStatus, QueueStateResponse, Song};
use crate::voice::state::GuildMusicState;

pub struct VoiceManager {
    songbird: Arc<Songbird>,
    cache: Arc<Cache>,
    states: Arc<RwLock<HashMap<u64, Arc<RwLock<GuildMusicState>>>>>,
    http_client: Client,
    db_pool: SqlitePool,
    default_volume: f32,
}

impl VoiceManager {
    pub fn new(
        songbird: Arc<Songbird>,
        cache: Arc<Cache>,
        db_pool: SqlitePool,
        default_volume: f32,
    ) -> Self {
        Self {
            songbird,
            cache,
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
            state.transition_to(PlaybackStatus::Idle, "joined voice channel");
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

    pub async fn leave_channel(
        &self,
        guild_id: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Voice] Leaving guild {}", guild_id);
        self.stop(guild_id).await?;
        let gid = GuildId::new(guild_id);
        let _ = self.songbird.leave(gid).await;
        let _ = self.songbird.remove(gid).await;
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        state.transition_to(PlaybackStatus::Disconnected, "left voice channel");
        state.channel_id = None;
        state.track_handle = None;
        state.current_song = None;
        state.queue.clear();
        state.queue_index = -1;
        state.skip_votes.clear();
        state.pause_votes.clear();
        state.resume_votes.clear();
        state.stop_votes.clear();
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
            state.transition_to(PlaybackStatus::Disconnected, "not connected to voice channel");
            state.track_handle = None;
            return Ok(());
        }

        let state_arc = self.get_or_create_state(guild_id).await;
        let next_song = {
            let mut state = state_arc.write().await;
            let next_idx = state.queue_index + 1;
            if next_idx >= 0 && (next_idx as usize) < state.queue.len() {
                state.queue_index = next_idx;
                let s = state.queue[next_idx as usize].clone();
                state.current_song = Some(s.clone());
                state.transition_to(PlaybackStatus::Loading, format!("preparing track '{}'", s.title));
                Some(s)
            } else if state.loop_enabled && !state.queue.is_empty() {
                state.queue_index = 0;
                let s = state.queue[0].clone();
                state.current_song = Some(s.clone());
                state.transition_to(PlaybackStatus::Loading, format!("looping playlist, preparing track '{}'", s.title));
                Some(s)
            } else {
                state.transition_to(PlaybackStatus::Idle, "queue completed (no more tracks)");
                state.track_handle = None;
                state.current_song = None;
                state.queue_index = -1;
                None
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
        {
            let state_arc = self.get_or_create_state(guild_id).await;
            let mut state = state_arc.write().await;
            state.transition_to(PlaybackStatus::Loading, format!("buffering stream for '{}'", song.title));
        }
        let mut ytdl = std::process::Command::new("yt-dlp")
            .args([
                "--extractor-args",
                "youtube:player_client=android,ios,mweb,web",
                "-o",
                "-",
                "-f",
                "ba[acodec=opus]/ba[ext=m4a]/ba/bestaudio/best",
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
                "-vn",
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

        let mut container = songbird::input::ChildContainer::from(vec![ytdl, ffmpeg]);

        // Pre-buffer ~0.5s of audio (192,000 bytes of f32le stereo 48kHz PCM) in a worker thread
        // before attaching to the Songbird mixer. This eliminates mixer thread blocking on yt-dlp startup,
        // clock-skew frame drops, and prevents the initial 5 seconds from skipping in Discord.
        let (initial_bytes, container) = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut initial = Vec::with_capacity(192_000);
            let mut temp_buf = [0u8; 16384];
            while initial.len() < 192_000 {
                match container.read(&mut temp_buf) {
                    Ok(0) => break,
                    Ok(n) => initial.extend_from_slice(&temp_buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        warn!("[Voice] Error pre-buffering audio: {}", e);
                        break;
                    }
                }
            }
            (initial, container)
        })
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Verify if playback state was cancelled/changed during pre-buffer
        let state_arc = self.get_or_create_state(guild_id).await;
        {
            let state = state_arc.read().await;
            if let Some(curr) = &state.current_song {
                if curr.url != song.url {
                    info!("[Voice] Song changed while pre-buffering '{}', abandoning stream", song.title);
                    return Ok(());
                }
            }
        }

        use std::io::Read;
        let reader = std::io::Cursor::new(initial_bytes).chain(container);
        let raw_adapter = songbird::input::RawAdapter::new(
            symphonia::core::io::ReadOnlySource::new(reader),
            48000,
            2,
        );
        let input: songbird::input::Input = raw_adapter.into();

        let handle = {
            let mut handler = handler_lock.lock().await;
            handler.play_only_input(input)
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
            state.transition_to(PlaybackStatus::Playing, format!("playing track '{}'", song.title));
            state.skip_votes.clear();
            state.pause_votes.clear();
            state.resume_votes.clear();
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
                        for mut song in songs {
                            song.requester_name = Some("AI Auto-Queue".to_string());
                            song.requester_handle = Some("AI Auto-Queue".to_string());
                            song.requester_avatar = Some("/logo_icon.svg".to_string());
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
        state.transition_to(PlaybackStatus::Paused, "paused by user");
        state.pause_votes.clear();
        Ok(())
    }

    pub async fn resume(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if let Some(h) = &state.track_handle {
            let _ = h.play();
        }
        state.transition_to(PlaybackStatus::Playing, "resumed by user");
        state.resume_votes.clear();
        Ok(())
    }

    pub async fn stop(&self, guild_id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;
        if let Some(h) = &state.track_handle {
            let _ = h.stop();
        }
        state.transition_to(PlaybackStatus::Idle, "stopped by user");
        state.track_handle = None;
        state.queue.clear();
        state.current_song = None;
        state.queue_index = -1;
        state.skip_votes.clear();
        state.pause_votes.clear();
        state.resume_votes.clear();
        state.stop_votes.clear();
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

    pub fn get_voice_participants(&self, guild_id: u64, channel_id: Option<u64>) -> Vec<u64> {
        let channel_id = match channel_id {
            Some(cid) => cid,
            None => return Vec::new(),
        };
        let target_chan = ChannelId::new(channel_id);

        if let Some(guild) = self.cache.guild(GuildId::new(guild_id)) {
            let bot_id = self.cache.current_user().id;
            let mut participants = Vec::new();
            for (uid, vs) in &guild.voice_states {
                if vs.channel_id == Some(target_chan) && *uid != bot_id {
                    let is_bot = if let Some(m) = &vs.member {
                        m.user.bot
                    } else if let Some(m) = guild.members.get(uid) {
                        m.user.bot
                    } else {
                        false
                    };
                    if !is_bot {
                        participants.push(uid.get());
                    }
                }
            }
            participants
        } else {
            Vec::new()
        }
    }

    pub fn calculate_required_votes(&self, participant_count: usize) -> u32 {
        if participant_count == 0 {
            return 1;
        }
        let ratio = 0.5;
        let base_required = (participant_count as f64 * ratio).floor() as u32 + 1;
        base_required.max(1).min(participant_count as u32)
    }

    pub async fn get_active_channel_id(&self, guild_id: u64) -> Option<u64> {
        if let Some(guild) = self.cache.guild(GuildId::new(guild_id)) {
            let bot_id = self.cache.current_user().id;
            if let Some(vs) = guild.voice_states.get(&bot_id) {
                if let Some(cid) = vs.channel_id {
                    return Some(cid.get());
                }
            }
        }
        let state_arc = self.get_or_create_state(guild_id).await;
        let state = state_arc.read().await;
        state.channel_id
    }

    pub async fn get_queue_state(&self, guild_id: u64) -> QueueStateResponse {
        let channel_id = self.get_active_channel_id(guild_id).await;
        let participants = self.get_voice_participants(guild_id, channel_id);
        let req_votes = self.calculate_required_votes(participants.len());

        let state_arc = self.get_or_create_state(guild_id).await;
        let state = state_arc.read().await;
        let connected = self.is_connected(guild_id).await;
        state.to_response(connected, req_votes).await
    }

    pub async fn vote_skip(&self, guild_id: u64, user_id: u64) -> (bool, u32, u32) {
        let channel_id = self.get_active_channel_id(guild_id).await;
        let participants = self.get_voice_participants(guild_id, channel_id);
        let req = self.calculate_required_votes(participants.len());

        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;

        if !participants.is_empty() {
            state.skip_votes.retain(|id| participants.contains(id));
        }

        if state.skip_votes.contains(&user_id) {
            state.skip_votes.remove(&user_id);
            info!("[Vote] Guild {}: User {} removed skip vote ({}/{})", guild_id, user_id, state.skip_votes.len(), req);
            (false, state.skip_votes.len() as u32, req)
        } else {
            state.skip_votes.insert(user_id);
            let count = state.skip_votes.len() as u32;
            info!("[Vote] Guild {}: User {} voted to skip ({}/{})", guild_id, user_id, count, req);
            if count >= req {
                state.skip_votes.clear();
                (true, count, req)
            } else {
                (false, count, req)
            }
        }
    }

    pub async fn vote_pause(&self, guild_id: u64, user_id: u64) -> (bool, u32, u32) {
        let channel_id = self.get_active_channel_id(guild_id).await;
        let participants = self.get_voice_participants(guild_id, channel_id);
        let req = self.calculate_required_votes(participants.len());

        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;

        if !participants.is_empty() {
            state.pause_votes.retain(|id| participants.contains(id));
        }

        if state.pause_votes.contains(&user_id) {
            state.pause_votes.remove(&user_id);
            info!("[Vote] Guild {}: User {} removed pause vote ({}/{})", guild_id, user_id, state.pause_votes.len(), req);
            (false, state.pause_votes.len() as u32, req)
        } else {
            state.pause_votes.insert(user_id);
            let count = state.pause_votes.len() as u32;
            info!("[Vote] Guild {}: User {} voted to pause ({}/{})", guild_id, user_id, count, req);
            if count >= req {
                state.pause_votes.clear();
                (true, count, req)
            } else {
                (false, count, req)
            }
        }
    }

    pub async fn vote_resume(&self, guild_id: u64, user_id: u64) -> (bool, u32, u32) {
        let channel_id = self.get_active_channel_id(guild_id).await;
        let participants = self.get_voice_participants(guild_id, channel_id);
        let req = self.calculate_required_votes(participants.len());

        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;

        if !participants.is_empty() {
            state.resume_votes.retain(|id| participants.contains(id));
        }

        if state.resume_votes.contains(&user_id) {
            state.resume_votes.remove(&user_id);
            info!("[Vote] Guild {}: User {} removed resume vote ({}/{})", guild_id, user_id, state.resume_votes.len(), req);
            (false, state.resume_votes.len() as u32, req)
        } else {
            state.resume_votes.insert(user_id);
            let count = state.resume_votes.len() as u32;
            info!("[Vote] Guild {}: User {} voted to resume ({}/{})", guild_id, user_id, count, req);
            if count >= req {
                state.resume_votes.clear();
                (true, count, req)
            } else {
                (false, count, req)
            }
        }
    }

    pub async fn vote_stop(&self, guild_id: u64, user_id: u64) -> (bool, u32, u32) {
        let channel_id = self.get_active_channel_id(guild_id).await;
        let participants = self.get_voice_participants(guild_id, channel_id);
        let req = self.calculate_required_votes(participants.len());

        let state_arc = self.get_or_create_state(guild_id).await;
        let mut state = state_arc.write().await;

        if !participants.is_empty() {
            state.stop_votes.retain(|id| participants.contains(id));
        }

        if state.stop_votes.contains(&user_id) {
            state.stop_votes.remove(&user_id);
            info!("[Vote] Guild {}: User {} removed stop vote ({}/{})", guild_id, user_id, state.stop_votes.len(), req);
            (false, state.stop_votes.len() as u32, req)
        } else {
            state.stop_votes.insert(user_id);
            let count = state.stop_votes.len() as u32;
            info!("[Vote] Guild {}: User {} voted to stop ({}/{})", guild_id, user_id, count, req);
            if count >= req {
                state.stop_votes.clear();
                (true, count, req)
            } else {
                (false, count, req)
            }
        }
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
