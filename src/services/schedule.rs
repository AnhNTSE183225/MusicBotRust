use std::sync::Arc;
use serenity::all::{
    ChannelId, ChannelType, Colour, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter,
    CreateMessage, GuildId, Http, UserId,
};
use sqlx::SqlitePool;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::db;
use crate::models::ScheduleEvent;

pub struct ScheduleService {
    pool: SqlitePool,
    http: Arc<Http>,
    frontend_url: String,
}

impl ScheduleService {
    pub fn new(pool: SqlitePool, http: Arc<Http>, frontend_url: String) -> Self {
        Self {
            pool,
            http,
            frontend_url: frontend_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn start_background_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                if let Err(e) = self.check_upcoming_reminders().await {
                    warn!("[Schedule] Error checking reminders: {}", e);
                }
                sleep(Duration::from_secs(30)).await;
            }
        });
    }

    async fn check_upcoming_reminders(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let events = db::get_all_active_schedule_events(&self.pool).await;
        let now = chrono::Utc::now();

        for ev in events {
            if let Ok(start_dt) = chrono::DateTime::parse_from_rfc3339(&ev.start) {
                let start_utc = start_dt.with_timezone(&chrono::Utc);
                let diff = start_utc - now;
                let minutes = diff.num_minutes();

                // 1. Lead reminder (starting in 1 to 15 minutes)
                if minutes > 0 && minutes <= 15 {
                    if !db::is_occurrence_reminded(&self.pool, &ev.id, "base", "lead").await {
                        self.send_reminder(&ev, &start_utc, "lead", 15).await;
                        db::mark_occurrence_reminded(&self.pool, &ev.id, "base", "lead").await;
                    }
                }

                // 2. Starting now (within 0 to 2 minutes ago)
                if minutes >= -2 && minutes <= 0 {
                    if !db::is_occurrence_reminded(&self.pool, &ev.id, "base", "start").await {
                        self.send_reminder(&ev, &start_utc, "start", 0).await;
                        db::mark_occurrence_reminded(&self.pool, &ev.id, "base", "start").await;
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn send_event_invitations(&self, event: &ScheduleEvent, recipient_ids: Option<&[String]>) -> usize {
        let gid_u64: u64 = match event.guild_id.parse() {
            Ok(g) => g,
            Err(_) => return 0,
        };
        let gid = GuildId::new(gid_u64);

        let event_url = format!("{}/?tab=schedule&guild={}&event={}", self.frontend_url, event.guild_id, event.id);
        let embed = self.build_event_invite_embed(event, &event_url);

        let organizer_mention = if !event.organizer_id.is_empty() {
            format!("<@{}>", event.organizer_id)
        } else {
            event.organizer_name.clone().unwrap_or_else(|| "Someone".to_string())
        };
        let event_title = if event.title.is_empty() { "Scheduled Event" } else { &event.title };
        let content = format!(
            "{} invited you to an event: **{}**\nAccept or view details: {}",
            organizer_mention, event_title, event_url
        );

        let mut sent_count = 0;

        // If specific recipients were requested (e.g. newly added users)
        if let Some(recipients) = recipient_ids {
            for uid_str in recipients {
                if uid_str == &event.organizer_id {
                    continue;
                }
                if let Ok(uid_u64) = uid_str.parse::<u64>() {
                    let user_id = UserId::new(uid_u64);
                    let msg = CreateMessage::new().content(&content).embed(embed.clone());
                    match user_id.create_dm_channel(&self.http).await {
                        Ok(dm) => {
                            if let Err(e) = dm.send_message(&self.http, msg).await {
                                warn!("[Schedule] Could not send event invite DM to {}: {}", uid_str, e);
                            } else {
                                sent_count += 1;
                            }
                        }
                        Err(e) => {
                            warn!("[Schedule] Could not create DM channel for {}: {}", uid_str, e);
                        }
                    }
                }
            }
            return sent_count;
        }

        // Server-wide event -> post to target text channel
        if event.invite_everyone {
            if let Some(target_cid) = self.find_schedule_target_channel(gid, "event").await {
                let allowed_mentions = CreateAllowedMentions::new().everyone(false).all_users(true);
                let msg = CreateMessage::new()
                    .content(&content)
                    .embed(embed)
                    .allowed_mentions(allowed_mentions);

                match target_cid.send_message(&self.http, msg).await {
                    Ok(_) => {
                        info!("[Schedule] Sent event invitation for {} to channel {}", event.id, target_cid);
                        sent_count += 1;
                    }
                    Err(e) => {
                        warn!("[Schedule] Failed to send event invitation to channel {}: {}", target_cid, e);
                    }
                }
            } else {
                warn!("[Schedule] No suitable text channel found in guild {} to post invitation", gid);
            }
        } else {
            // Direct DMs to invitees
            for uid_str in &event.invitee_ids {
                if uid_str == &event.organizer_id {
                    continue;
                }
                if let Ok(uid_u64) = uid_str.parse::<u64>() {
                    let user_id = UserId::new(uid_u64);
                    let msg = CreateMessage::new().content(&content).embed(embed.clone());
                    match user_id.create_dm_channel(&self.http).await {
                        Ok(dm) => {
                            if let Err(e) = dm.send_message(&self.http, msg).await {
                                warn!("[Schedule] Could not send event invite DM to {}: {}", uid_str, e);
                            } else {
                                sent_count += 1;
                            }
                        }
                        Err(e) => {
                            warn!("[Schedule] Could not create DM channel for {}: {}", uid_str, e);
                        }
                    }
                }
            }
            info!("[Schedule] Sent {} event invite DMs for {}", sent_count, event.id);
        }

        sent_count
    }

    pub async fn send_event_update_notifications(
        &self,
        old_event: &ScheduleEvent,
        updated_event: &ScheduleEvent,
        editor_id: Option<&str>,
    ) -> usize {
        let mut sent_count = 0;
        let event_id = &updated_event.id;

        // 1. Newly added persons get a fresh invitation message
        let old_invitees: std::collections::HashSet<&str> = if !old_event.invite_everyone {
            old_event.invitee_ids.iter().map(|s| s.as_str()).collect()
        } else {
            std::collections::HashSet::new()
        };

        let added_invitee_ids: Vec<String> = if !updated_event.invite_everyone {
            updated_event
                .invitee_ids
                .iter()
                .filter(|uid| !old_invitees.contains(uid.as_str()) && uid.as_str() != updated_event.organizer_id.as_str())
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        if !added_invitee_ids.is_empty() {
            sent_count += self.send_event_invitations(updated_event, Some(&added_invitee_ids)).await;
        }

        // 2. Check if time/recurrence/duration changed
        let time_changed = old_event.start != updated_event.start
            || old_event.duration_minutes != updated_event.duration_minutes
            || serde_json::to_string(&old_event.recurrence).unwrap_or_default()
                != serde_json::to_string(&updated_event.recurrence).unwrap_or_default();

        if time_changed {
            db::clear_event_reminders(&self.pool, event_id).await;

            let gid_u64: u64 = match updated_event.guild_id.parse() {
                Ok(g) => g,
                Err(_) => return sent_count,
            };
            let gid = GuildId::new(gid_u64);

            let event_url = format!("{}/?tab=schedule&guild={}&event={}", self.frontend_url, updated_event.guild_id, updated_event.id);
            let embed = self.build_event_update_embed(updated_event, &event_url, Some(&old_event.start));
            let event_title = if updated_event.title.is_empty() { "Scheduled Event" } else { &updated_event.title };

            if updated_event.invite_everyone {
                if let Some(target_cid) = self.find_schedule_target_channel(gid, "event").await {
                    let mut dt_str = String::new();
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&updated_event.start) {
                        let unix_ts = dt.timestamp();
                        dt_str = format!("\nNew time: <t:{}:F> (<t:{}:R>)", unix_ts, unix_ts);
                    }
                    let content = format!(
                        "📢 The event **{}** has been rescheduled!{}\nAccept or view details: {}",
                        event_title, dt_str, event_url
                    );
                    let allowed_mentions = CreateAllowedMentions::new().everyone(false).all_users(true);
                    let msg = CreateMessage::new().content(content).embed(embed).allowed_mentions(allowed_mentions);
                    if let Ok(_) = target_cid.send_message(&self.http, msg).await {
                        sent_count += 1;
                    }
                }
            } else {
                let added_set: std::collections::HashSet<&str> = added_invitee_ids.iter().map(|s| s.as_str()).collect();
                let existing_invitees: Vec<&str> = updated_event
                    .invitee_ids
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|uid| !added_set.contains(uid))
                    .collect();

                let mut recipients_to_notify: Vec<&str> = Vec::new();
                for uid in &existing_invitees {
                    if editor_id.map(|ed| ed == *uid).unwrap_or(false) {
                        continue;
                    }
                    recipients_to_notify.push(uid);
                }

                if !updated_event.organizer_id.is_empty()
                    && editor_id.map(|ed| ed != updated_event.organizer_id).unwrap_or(true)
                    && !recipients_to_notify.contains(&updated_event.organizer_id.as_str())
                {
                    recipients_to_notify.push(&updated_event.organizer_id);
                }

                let content = format!(
                    "📢 The event **{}** has been rescheduled.\nCheck new time & details: {}",
                    event_title, event_url
                );

                for uid_str in recipients_to_notify {
                    if let Ok(uid_u64) = uid_str.parse::<u64>() {
                        let user_id = UserId::new(uid_u64);
                        let msg = CreateMessage::new().content(&content).embed(embed.clone());
                        if let Ok(dm) = user_id.create_dm_channel(&self.http).await {
                            if let Ok(_) = dm.send_message(&self.http, msg).await {
                                sent_count += 1;
                            }
                        }
                    }
                }
            }
        }

        sent_count
    }

    async fn send_reminder(&self, event: &ScheduleEvent, start_dt: &chrono::DateTime<chrono::Utc>, r_type: &str, lead_mins: i64) {
        let gid_u64: u64 = match event.guild_id.parse() {
            Ok(g) => g,
            Err(_) => return,
        };
        let gid = GuildId::new(gid_u64);
        let event_url = format!("{}/?tab=schedule&guild={}&event={}", self.frontend_url, event.guild_id, event.id);

        let embed = self.build_reminder_embed(event, start_dt, r_type, lead_mins);

        let channel_content = if r_type == "start" {
            "🎉 **Event starting now!**"
        } else {
            &format!("📢 **Upcoming Event in {} minutes!**", lead_mins)
        };

        let dm_content = if r_type == "start" {
            format!("🎉 **Starting Now**: Your scheduled event '{}' is starting right now!\nJoin: {}", event.title, event_url)
        } else {
            format!("⏰ **Reminder**: Your scheduled event '{}' starts in {} minutes!\nDetails: {}", event.title, lead_mins, event_url)
        };

        if event.invite_everyone {
            if let Some(target_cid) = self.find_schedule_target_channel(gid, "announcement").await {
                let allowed_mentions = CreateAllowedMentions::new().everyone(false).all_users(true);
                let msg = CreateMessage::new().content(channel_content).embed(embed).allowed_mentions(allowed_mentions);
                let _ = target_cid.send_message(&self.http, msg).await;
            }
        } else {
            let mut recipients = std::collections::HashSet::new();
            if !event.organizer_id.is_empty() {
                recipients.insert(event.organizer_id.clone());
            }
            for uid in &event.invitee_ids {
                recipients.insert(uid.clone());
            }

            // Exclude declined responses
            for uid in recipients {
                let is_declined = event.series_responses.get(&uid).and_then(|v| v.as_str()) == Some("declined")
                    || event.occurrence_responses.get("base").and_then(|o| o.get(&uid)).and_then(|v| v.as_str()) == Some("declined");

                if !is_declined {
                    if let Ok(uid_u64) = uid.parse::<u64>() {
                        let user_id = UserId::new(uid_u64);
                        let msg = CreateMessage::new().content(&dm_content).embed(embed.clone());
                        if let Ok(dm) = user_id.create_dm_channel(&self.http).await {
                            let _ = dm.send_message(&self.http, msg).await;
                        }
                    }
                }
            }
        }
    }

    async fn find_schedule_target_channel(&self, gid: GuildId, preferred_channel_type: &str) -> Option<ChannelId> {
        let channels = match self.http.get_channels(gid).await {
            Ok(c) => c,
            Err(e) => {
                warn!("[Schedule] Failed to fetch channels for guild {}: {}", gid, e);
                return None;
            }
        };

        let event_name = "event";
        let announcement_name = "general";

        let candidate_names = if preferred_channel_type == "announcement" {
            vec![announcement_name, event_name]
        } else {
            vec![event_name, announcement_name]
        };

        for name in candidate_names {
            for ch in &channels {
                if ch.kind == ChannelType::Text {
                    let n = ch.name.to_lowercase();
                    if n == name || n == format!("{}s", name) {
                        return Some(ch.id);
                    }
                }
            }
        }

        // Fallback: first text channel
        channels.iter().find(|ch| ch.kind == ChannelType::Text).map(|ch| ch.id)
    }

    fn build_event_invite_embed(&self, event: &ScheduleEvent, event_url: &str) -> CreateEmbed {
        let title = if event.title.is_empty() { "Scheduled Event" } else { &event.title };
        let emoji = self.get_category_emoji(event.category.as_deref());
        let category = event.category.as_deref().unwrap_or("other");
        let category_cap = self.capitalize(category);

        let desc = if event.description.is_empty() {
            "You have been invited to join this event!".to_string()
        } else {
            event.description.clone()
        };

        let mut embed = CreateEmbed::new()
            .title(format!("✉️ {} New Event Invitation: {}", emoji, title))
            .description(desc)
            .colour(Colour::from_rgb(114, 137, 218)); // Discord Blurple

        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&event.start) {
            let unix_ts = dt.timestamp();
            embed = embed.field("Start Time", format!("<t:{}:F> (<t:{}:R>)", unix_ts, unix_ts), true);
        } else {
            embed = embed.field("Start Time", &event.start, true);
        }

        embed = embed.field("Duration", format!("{} mins", event.duration_minutes), true);
        embed = embed.field("Category", format!("{} {}", emoji, category_cap), true);

        if let Some(loc) = &event.location {
            match loc.kind.as_str() {
                "discord" => {
                    if let Some(cid) = &loc.channel_id {
                        embed = embed.field("Location", format!("🔊 <#{}>", cid), false);
                    }
                }
                "address" => {
                    if let Some(addr) = &loc.address {
                        embed = embed.field("Location", format!("📍 {}", addr), false);
                    }
                }
                "link" => {
                    if let Some(u) = &loc.url {
                        embed = embed.field("Location", format!("🔗 [Join Link]({})", u), false);
                    }
                }
                _ => {}
            }
        }

        if let Some(rec) = &event.recurrence {
            if !rec.freq.is_empty() && rec.freq != "none" {
                let freq_cap = self.capitalize(&rec.freq);
                embed = embed.field("Recurrence", format!("🔄 Repeats {}", freq_cap), true);
            }
        }

        embed = embed.field(
            "RSVP & Details",
            format!("👉 [**Accept / View on MusicBỏTúi**]({})", event_url),
            false,
        );

        if let Some(img) = &event.image_url {
            if !img.trim().is_empty() {
                embed = embed.image(img.trim());
            }
        }

        let organizer = event.organizer_name.as_deref().unwrap_or("Server Member");
        embed = embed.footer(CreateEmbedFooter::new(format!("Organized by {}", organizer)));

        embed
    }

    fn build_event_update_embed(&self, event: &ScheduleEvent, event_url: &str, old_start_str: Option<&str>) -> CreateEmbed {
        let title = if event.title.is_empty() { "Scheduled Event" } else { &event.title };
        let emoji = self.get_category_emoji(event.category.as_deref());
        let category = event.category.as_deref().unwrap_or("other");
        let category_cap = self.capitalize(category);

        let desc = if event.description.is_empty() {
            "The scheduled time for this event has been updated.".to_string()
        } else {
            event.description.clone()
        };

        let mut embed = CreateEmbed::new()
            .title(format!("🔄 {} Event Rescheduled: {}", emoji, title))
            .description(desc)
            .colour(Colour::from_rgb(255, 170, 0)); // Warning amber

        if let Some(old_s) = old_start_str {
            if let Ok(dt_old) = chrono::DateTime::parse_from_rfc3339(old_s) {
                let old_unix = dt_old.timestamp();
                embed = embed.field("Previous Time", format!("<t:{}:F>", old_unix), true);
            } else {
                embed = embed.field("Previous Time", old_s, true);
            }
        }

        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&event.start) {
            let unix_ts = dt.timestamp();
            embed = embed.field("New Start Time", format!("<t:{}:F> (<t:{}:R>)", unix_ts, unix_ts), true);
        } else {
            embed = embed.field("New Start Time", &event.start, true);
        }

        embed = embed.field("Duration", format!("{} mins", event.duration_minutes), true);
        embed = embed.field("Category", format!("{} {}", emoji, category_cap), true);

        if let Some(loc) = &event.location {
            match loc.kind.as_str() {
                "discord" => {
                    if let Some(cid) = &loc.channel_id {
                        embed = embed.field("Location", format!("🔊 <#{}>", cid), false);
                    }
                }
                "address" => {
                    if let Some(addr) = &loc.address {
                        embed = embed.field("Location", format!("📍 {}", addr), false);
                    }
                }
                "link" => {
                    if let Some(u) = &loc.url {
                        embed = embed.field("Location", format!("🔗 [Join Link]({})", u), false);
                    }
                }
                _ => {}
            }
        }

        embed = embed.field(
            "RSVP & Details",
            format!("👉 [**Accept / View on MusicBỏTúi**]({})", event_url),
            false,
        );

        if let Some(img) = &event.image_url {
            if !img.trim().is_empty() {
                embed = embed.image(img.trim());
            }
        }

        let organizer = event.organizer_name.as_deref().unwrap_or("Server Member");
        embed = embed.footer(CreateEmbedFooter::new(format!("Organized by {}", organizer)));

        embed
    }

    fn build_reminder_embed(&self, event: &ScheduleEvent, start_dt: &chrono::DateTime<chrono::Utc>, r_type: &str, lead_mins: i64) -> CreateEmbed {
        let title = if event.title.is_empty() { "Scheduled Event" } else { &event.title };
        let emoji = self.get_category_emoji(event.category.as_deref());
        let category = event.category.as_deref().unwrap_or("other");
        let category_cap = self.capitalize(category);

        let (title_text, desc, colour) = if r_type == "start" {
            (
                format!("🎉 {} Starting Now: {}", emoji, title),
                if event.description.is_empty() { "This event is starting right now!".to_string() } else { event.description.clone() },
                Colour::from_rgb(46, 204, 113), // Emerald Green
            )
        } else {
            (
                format!("⏰ {} Starting in {}m: {}", emoji, lead_mins, title),
                if event.description.is_empty() { format!("This event will start in {} minutes!", lead_mins) } else { event.description.clone() },
                Colour::from_rgb(52, 152, 219), // Blue
            )
        };

        let unix_ts = start_dt.timestamp();
        let mut embed = CreateEmbed::new()
            .title(title_text)
            .description(desc)
            .colour(colour)
            .field("Start Time", format!("<t:{}:F> (<t:{}:R>)", unix_ts, unix_ts), true)
            .field("Duration", format!("{} mins", event.duration_minutes), true)
            .field("Category", format!("{} {}", emoji, category_cap), true);

        if let Some(loc) = &event.location {
            match loc.kind.as_str() {
                "discord" => {
                    if let Some(cid) = &loc.channel_id {
                        embed = embed.field("Location", format!("🔊 <#{}>", cid), false);
                    }
                }
                "address" => {
                    if let Some(addr) = &loc.address {
                        embed = embed.field("Location", format!("📍 {}", addr), false);
                    }
                }
                "link" => {
                    if let Some(u) = &loc.url {
                        embed = embed.field("Location", format!("🔗 [Join Link]({})", u), false);
                    }
                }
                _ => {}
            }
        }

        let event_url = format!("{}/?tab=schedule&guild={}&event={}", self.frontend_url, event.guild_id, event.id);
        embed = embed.field(
            "RSVP & Details",
            format!("👉 [**View on MusicBỏTúi**]({})", event_url),
            false,
        );

        if let Some(img) = &event.image_url {
            if !img.trim().is_empty() {
                embed = embed.image(img.trim());
            }
        }

        let organizer = event.organizer_name.as_deref().unwrap_or("Server Member");
        embed = embed.footer(CreateEmbedFooter::new(format!("Organized by {}", organizer)));

        embed
    }

    fn get_category_emoji(&self, category: Option<&str>) -> &'static str {
        match category.unwrap_or("other") {
            "gaming" => "🎮",
            "food" => "🍴",
            "outdoor" => "📍",
            _ => "📅",
        }
    }

    fn capitalize(&self, s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        }
    }
}
