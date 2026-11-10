use std::sync::Arc;
use serenity::all::Http;
use sqlx::{Row, SqlitePool};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

pub struct ScheduleService {
    pool: SqlitePool,
    #[allow(dead_code)]
    http: Arc<Http>,
}

impl ScheduleService {
    pub fn new(pool: SqlitePool, http: Arc<Http>) -> Self {
        Self { pool, http }
    }

    pub fn start_background_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                if let Err(e) = self.check_upcoming_reminders().await {
                    warn!("[Schedule] Error checking reminders: {}", e);
                }
                sleep(Duration::from_secs(60)).await;
            }
        });
    }

    async fn check_upcoming_reminders(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Query upcoming events across guilds
        let rows = sqlx::query(
            "SELECT id, guild_id, title, start, organizer_id FROM guild_schedule_events"
        )
        .fetch_all(&self.pool)
        .await?;

        let now = chrono::Utc::now();
        for r in rows {
            let title: String = r.get("title");
            let start_str: String = r.get("start");

            if let Ok(start_dt) = chrono::DateTime::parse_from_rfc3339(&start_str) {
                let diff = start_dt.with_timezone(&chrono::Utc) - now;
                let minutes = diff.num_minutes();
                if minutes > 0 && minutes <= 15 {
                    info!("[Schedule] Upcoming event '{}' starting in {} minutes", title, minutes);
                }
            }
        }

        Ok(())
    }
}
