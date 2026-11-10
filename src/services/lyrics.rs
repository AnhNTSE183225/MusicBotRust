use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricWord {
    pub begin: f64,
    pub end: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_space: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    pub begin: f64,
    pub end: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<LyricWord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormattedLyrics {
    #[serde(rename = "type")]
    pub lyrics_type: String, // "line" | "syllable"
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro_offset: Option<f64>,
    pub lines: Vec<LyricLine>,
}

pub struct LyricsService {
    client: Client,
    cache: Arc<Mutex<HashMap<String, (FormattedLyrics, f64)>>>,
}

pub fn extract_video_id(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.len() == 11 && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(trimmed.to_string());
    }
    let re = regex::Regex::new(r"(?:v=|/|youtu\.be/|embed/|shorts/)([0-9A-Za-z_-]{11})(?:[&?/\b]|$)").ok()?;
    re.captures(trimmed).and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

pub async fn get_sponsorblock_intro_offset(client: &Client, video_id: &str) -> Option<f64> {
    if video_id.is_empty() {
        return None;
    }
    let encoded_id = url::form_urlencoded::byte_serialize(video_id.as_bytes()).collect::<String>();
    let url = format!(
        "https://sponsor.ajay.app/api/skipSegments?videoID={}&categories=[%22music_offtopic%22]",
        encoded_id
    );

    let res = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.get(&url).header("User-Agent", "MusicBot/1.0").send(),
    )
    .await
    .ok()?
    .ok()?;

    if !res.status().is_success() {
        return None;
    }

    let val = res.json::<serde_json::Value>().await.ok()?;
    if let Some(arr) = val.as_array() {
        for seg_obj in arr {
            if let Some(seg) = seg_obj.get("segment").and_then(|s| s.as_array()) {
                if seg.len() >= 2 {
                    let start = seg[0].as_f64().unwrap_or(0.0);
                    let end = seg[1].as_f64().unwrap_or(0.0);
                    if start <= 2.0 && end > start {
                        tracing::info!(
                            "[Lyrics] Found SponsorBlock MV intro segment for {}: {} -> {}",
                            video_id,
                            start,
                            end
                        );
                        return Some((end * 100.0).round() / 100.0);
                    }
                }
            }
        }
    }

    None
}

impl LyricsService {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("MusicBot/1.0 (https://github.com/theanh/MusicBot)")
                .build()
                .unwrap_or_default(),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_lyrics(
        &self,
        title: &str,
        artist: &str,
        duration: f64,
        video_id: Option<&str>,
    ) -> Option<FormattedLyrics> {
        let key = format!(
            "{}:{}:{}",
            video_id.unwrap_or(""),
            title.trim().to_lowercase(),
            artist.trim().to_lowercase()
        );
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();

        {
            let cache = self.cache.lock().await;
            if let Some((data, ts)) = cache.get(&key) {
                if now - ts < 3600.0 {
                    return Some(data.clone());
                }
            }
        }

        // Clean title for searching
        let clean_title = title
            .replace("[Official Video]", "")
            .replace("(Official Audio)", "")
            .replace("[Official Audio]", "")
            .replace("(Official Music Video)", "")
            .replace("[Official Music Video]", "")
            .replace("(Lyrics)", "")
            .replace("[Lyrics]", "");

        let url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}&duration={}",
            url::form_urlencoded::byte_serialize(clean_title.trim().as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(artist.trim().as_bytes()).collect::<String>(),
            duration as i64
        );

        let mut found_lyrics = None;

        if let Ok(res) = self.client.get(&url).send().await {
            if res.status().is_success() {
                if let Ok(val) = res.json::<serde_json::Value>().await {
                    if let Some(parsed) = Self::extract_lyrics_from_json(&val) {
                        found_lyrics = Some(parsed);
                    }
                }
            }
        }

        // Fallback search
        if found_lyrics.is_none() {
            let search_url = format!(
                "https://lrclib.net/api/search?q={}",
                url::form_urlencoded::byte_serialize(format!("{} {}", clean_title, artist).as_bytes()).collect::<String>()
            );

            if let Ok(res) = self.client.get(&search_url).send().await {
                if res.status().is_success() {
                    if let Ok(items) = res.json::<Vec<serde_json::Value>>().await {
                        for item in items.iter().take(3) {
                            if let Some(parsed) = Self::extract_lyrics_from_json(item) {
                                found_lyrics = Some(parsed);
                                break;
                            }
                        }
                    }
                }
            }
        }

        if let Some(mut parsed) = found_lyrics {
            if let Some(vid) = video_id {
                if let Some(offset) = get_sponsorblock_intro_offset(&self.client, vid).await {
                    parsed.intro_offset = Some(offset);
                }
            }
            let mut cache = self.cache.lock().await;
            cache.insert(key, (parsed.clone(), now));
            return Some(parsed);
        }

        None
    }

    fn extract_lyrics_from_json(val: &serde_json::Value) -> Option<FormattedLyrics> {
        let synced = val.get("syncedLyrics").and_then(|v| v.as_str());
        if let Some(s) = synced {
            if let Some(parsed) = Self::parse_lrc(s, "lrclib") {
                return Some(parsed);
            }
        }

        let plain = val.get("plainLyrics").and_then(|v| v.as_str());
        if let Some(p) = plain {
            if let Some(parsed) = Self::parse_plain(p, "lrclib") {
                return Some(parsed);
            }
        }

        None
    }

    fn parse_lrc(lrc_text: &str, provider: &str) -> Option<FormattedLyrics> {
        let re = regex::Regex::new(r"\[(\d{1,2}):(\d{2}(?:\.\d{1,3})?)\](.*)").ok()?;
        let mut entries = Vec::new();

        for line in lrc_text.lines() {
            let line = line.trim();
            if let Some(caps) = re.captures(line) {
                let min: f64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
                let sec: f64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
                let text = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("");
                if !text.is_empty() {
                    entries.push((min * 60.0 + sec, text.to_string()));
                }
            }
        }

        if entries.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        for (i, (sec, text)) in entries.iter().enumerate() {
            let end_sec = if i + 1 < entries.len() {
                (entries[i + 1].0).min(sec + 8.0)
            } else {
                sec + 5.0
            };

            lines.push(LyricLine {
                begin: (*sec * 1000.0).round() / 1000.0,
                end: (end_sec * 1000.0).round() / 1000.0,
                text: text.clone(),
                words: None,
            });
        }

        Some(FormattedLyrics {
            lyrics_type: "line".to_string(),
            provider: provider.to_string(),
            intro_offset: None,
            lines,
        })
    }

    fn parse_plain(plain_text: &str, provider: &str) -> Option<FormattedLyrics> {
        let mut lines = Vec::new();
        for (i, line) in plain_text.lines().enumerate() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let begin = (i as f64) * 4.0;
                lines.push(LyricLine {
                    begin,
                    end: begin + 4.0,
                    text: trimmed.to_string(),
                    words: None,
                });
            }
        }

        if lines.is_empty() {
            return None;
        }

        Some(FormattedLyrics {
            lyrics_type: "line".to_string(),
            provider: provider.to_string(),
            intro_offset: None,
            lines,
        })
    }
}
