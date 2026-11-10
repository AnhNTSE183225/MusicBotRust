use std::collections::HashSet;
use reqwest::Client;
use serde_json::json;
use sqlx::SqlitePool;
use tracing::{info, warn};
use crate::db;
use crate::models::Song;
use crate::services::youtube::extract_song_metadata;

pub struct KeyVerificationResult {
    pub status: String, // "healthy" | "quota_exceeded" | "invalid" | "error"
    pub message: String,
    pub retry_delay: f64,
}

pub async fn verify_gemini_api_key(client: &Client, api_key: &str) -> KeyVerificationResult {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
        api_key
    );

    let body = json!({
        "contents": [{
            "parts": [{ "text": "ping" }]
        }]
    });

    let res = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            return KeyVerificationResult {
                status: "error".to_string(),
                message: format!("Connection error: {}", e),
                retry_delay: 0.0,
            }
        }
    };

    let status = res.status();
    if status.is_success() {
        KeyVerificationResult {
            status: "healthy".to_string(),
            message: "Key is valid and active".to_string(),
            retry_delay: 0.0,
        }
    } else if status.as_u16() == 429 {
        KeyVerificationResult {
            status: "quota_exceeded".to_string(),
            message: "Quota exceeded for this key".to_string(),
            retry_delay: 3600.0,
        }
    } else {
        let err_text = res.text().await.unwrap_or_default();
        KeyVerificationResult {
            status: "invalid".to_string(),
            message: format!("Invalid key (HTTP {}): {}", status, err_text),
            retry_delay: 0.0,
        }
    }
}

pub async fn generate_search_queries(
    client: &Client,
    pool: &SqlitePool,
    prompt: &str,
    count: usize,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut attempts = 0;
    while attempts < 3 {
        attempts += 1;
        let api_key = match db::pick_healthy_gemini_key(pool).await {
            Some(k) => k,
            None => {
                warn!("[AI] No healthy Gemini API keys in pool, using fallback search queries");
                return Ok(vec![prompt.to_string()]);
            }
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
            api_key
        );

        let system_instruction = "You are a music DJ assistant. Generate a diverse list of distinct, real, high-quality songs matching the user's prompt or vibe. Return ONLY a raw JSON array of strings, where each string is in format 'Artist - Song Title'. Do not include markdown codeblocks or explanation.";
        let contents = json!({
            "contents": [
                {
                    "parts": [
                        { "text": format!("{}\nUser prompt: {}\nNumber of songs needed: {}", system_instruction, prompt, count) }
                    ]
                }
            ]
        });

        let res = match client.post(&url).json(&contents).send().await {
            Ok(r) => r,
            Err(e) => {
                warn!("[AI] Request failed with key {}: {}", db::mask_key(&api_key), e);
                continue;
            }
        };

        let status = res.status();
        if status.is_success() {
            db::mark_gemini_key_used(pool, &api_key).await;
            let val: serde_json::Value = res.json().await?;
            if let Some(text) = val.get("candidates")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("content"))
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.get(0))
                .and_then(|p| p.get("text"))
                .and_then(|t| t.as_str())
            {
                let clean = text.trim()
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();

                if let Ok(queries) = serde_json::from_str::<Vec<String>>(clean) {
                    if !queries.is_empty() {
                        return Ok(queries);
                    }
                }
            }
        } else if status.as_u16() == 429 {
            warn!("[AI] Key {} hit quota 429, marking on cooldown", db::mask_key(&api_key));
            db::mark_gemini_key_cooldown(pool, &api_key, 3600.0 * 2.0, "Quota exceeded 429").await;
        } else {
            let err_text = res.text().await.unwrap_or_default();
            warn!("[AI] Key {} error {}: {}", db::mask_key(&api_key), status, err_text);
            if status.as_u16() == 400 || status.as_u16() == 403 {
                db::mark_gemini_key_cooldown(pool, &api_key, 3600.0 * 24.0, "Auth error 400/403").await;
            }
        }
    }

    Ok(vec![prompt.to_string()])
}

pub async fn search_songs(
    queries: &[String],
    max_count: usize,
    recent_titles: &HashSet<String>,
) -> Vec<Song> {
    let mut songs = Vec::new();
    let mut seen_titles = recent_titles.clone();

    for query in queries {
        if songs.len() >= max_count {
            break;
        }

        if let Ok(song) = extract_song_metadata(query, None, Some("AI Assistant".to_string()), None).await {
            let norm = song.title.to_lowercase().trim().to_string();
            if !seen_titles.contains(&norm) {
                seen_titles.insert(norm);
                songs.push(song);
            }
        }
    }

    info!("[AI] Searched and resolved {} candidate songs", songs.len());
    songs
}
