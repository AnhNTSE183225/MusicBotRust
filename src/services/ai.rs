use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use regex::Regex;
use reqwest::{Client, StatusCode};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tracing::{info, warn};
use crate::db;
use crate::models::Song;
use crate::services::youtube::extract_song_metadata;

pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.8-flash";
const MODEL_OVERRIDE_TTL: Duration = Duration::from_secs(6 * 3600);

static CONFIGURED_MODEL: OnceLock<String> = OnceLock::new();
static RESOLVED_MODEL_CACHE: OnceLock<RwLock<Option<(String, Instant)>>> = OnceLock::new();

pub struct KeyVerificationResult {
    pub status: String, // "healthy" | "quota_exceeded" | "invalid" | "error"
    pub message: String,
    pub retry_delay: f64,
}

/// Registers the baseline Gemini model from configuration (called at startup).
pub fn configure_default_model(model: &str) {
    let normalized = normalize_model_name(model);
    if !normalized.is_empty() {
        let _ = CONFIGURED_MODEL.set(normalized);
    }
}

fn normalize_model_name(raw: &str) -> String {
    raw.trim().trim_start_matches("models/").trim().to_string()
}

fn model_cache() -> &'static RwLock<Option<(String, Instant)>> {
    RESOLVED_MODEL_CACHE.get_or_init(|| RwLock::new(None))
}

async fn get_active_model() -> String {
    {
        let guard = model_cache().read().await;
        if let Some((model, resolved_at)) = &*guard {
            if resolved_at.elapsed() < MODEL_OVERRIDE_TTL {
                return model.clone();
            }
        }
    }
    CONFIGURED_MODEL
        .get()
        .cloned()
        .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string())
}

async fn cache_resolved_model(model: &str) {
    let normalized = normalize_model_name(model);
    if normalized.is_empty() {
        return;
    }
    let mut guard = model_cache().write().await;
    *guard = Some((normalized, Instant::now()));
}

/// Extracts a replacement model name (e.g. `gemini-3.8-flash`) if Google includes one in a 404 message.
pub fn extract_model_hint_from_404(err_body: &str, failed_model: &str) -> Option<String> {
    let re = Regex::new(r"models/(gemini-[a-zA-Z0-9.\-]+)").ok()?;
    let failed_norm = normalize_model_name(failed_model);
    for caps in re.captures_iter(err_body) {
        if let Some(m) = caps.get(1) {
            let candidate = normalize_model_name(m.as_str());
            if !candidate.is_empty() && candidate != failed_norm {
                return Some(candidate);
            }
        }
    }
    None
}

fn parse_gemini_version(model_name: &str) -> (u32, u32) {
    let re = Regex::new(r"gemini-(\d+)(?:\.(\d+))?").expect("valid version regex");
    if let Some(caps) = re.captures(model_name) {
        let major = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
        let minor = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
        return (major, minor);
    }
    (0, 0)
}

fn score_gemini_candidate(name: &str) -> Option<(u8, u32, u32, u8, u8)> {
    let lower = name.to_lowercase();
    if !lower.starts_with("gemini-") {
        return None;
    }
    // Exclude non-text / specialized / preview-experimental endpoints when choosing a general model
    for excluded in ["embedding", "imagen", "veo", "tts", "audio", "robotics", "computer-use", "image"] {
        if lower.contains(excluded) {
            return None;
        }
    }

    let (major, minor) = parse_gemini_version(&lower);
    let tier = if lower.contains("-flash-lite") {
        2
    } else if lower.contains("-flash") {
        3
    } else if lower.contains("-pro") {
        1
    } else {
        0
    };
    if tier == 0 {
        return None;
    }

    let stability = if lower.contains("-exp") || lower.contains("-preview") {
        0
    } else {
        1
    };
    // Prefer exact base names (`gemini-3.8-flash`) over dated/latest snapshots when tied
    let clean_alias = if lower.ends_with("-flash") || lower.ends_with("-flash-lite") || lower.ends_with("-pro") {
        1
    } else {
        0
    };

    Some((stability, major, minor, tier, clean_alias))
}

/// Picks the best live text generation Gemini model from Google's `/v1beta/models` list payload.
pub fn select_best_gemini_model(
    models_json: &serde_json::Value,
    failed_model: Option<&str>,
    preferred_hint: Option<&str>,
) -> Option<String> {
    let failed_norm = failed_model.map(normalize_model_name).unwrap_or_default();
    let hint_norm = preferred_hint.map(normalize_model_name).filter(|s| !s.is_empty());

    let mut available_names = Vec::new();
    if let Some(models) = models_json.get("models").and_then(|m| m.as_array()) {
        for entry in models {
            let supports_generate = entry
                .get("supportedGenerationMethods")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|v| v.as_str() == Some("generateContent"))
                })
                .unwrap_or(false);
            if !supports_generate {
                continue;
            }
            if let Some(raw_name) = entry.get("name").and_then(|n| n.as_str()) {
                let name = normalize_model_name(raw_name);
                if !name.is_empty() && name != failed_norm {
                    available_names.push(name);
                }
            }
        }
    }

    if let Some(hint) = &hint_norm {
        if available_names.iter().any(|n| n == hint) {
            return Some(hint.clone());
        }
    }

    let best_ranked = available_names
        .iter()
        .filter_map(|n| score_gemini_candidate(n).map(|score| (score, n)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, n)| n.clone());

    best_ranked.or_else(|| {
        hint_norm.filter(|h| h != &failed_norm)
    })
}

async fn discover_replacement_model(
    client: &Client,
    api_key: &str,
    failed_model: &str,
    err_body: &str,
) -> Option<String> {
    let hint = extract_model_hint_from_404(err_body, failed_model);
    let list_url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}",
        api_key
    );

    if let Ok(resp) = client.get(&list_url).send().await {
        if resp.status().is_success() {
            if let Ok(json_val) = resp.json::<serde_json::Value>().await {
                if let Some(selected) = select_best_gemini_model(&json_val, Some(failed_model), hint.as_deref()) {
                    return Some(selected);
                }
            }
        }
    }

    hint
}

async fn post_generate_content(
    client: &Client,
    api_key: &str,
    model: &str,
    body: &serde_json::Value,
) -> Result<(StatusCode, String), reqwest::Error> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );
    let res = client.post(&url).json(body).send().await?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    Ok((status, text))
}

/// Calls Gemini `generateContent` using the configured/cached model, and on HTTP 404
/// automatically discovers a live replacement model via `/v1beta/models` and retries once.
async fn post_generate_content_with_fallback(
    client: &Client,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<(StatusCode, String, String), reqwest::Error> {
    let model = get_active_model().await;
    let (status, text) = post_generate_content(client, api_key, &model, body).await?;
    if status != StatusCode::NOT_FOUND {
        return Ok((status, text, model));
    }

    if let Some(replacement) = discover_replacement_model(client, api_key, &model, &text).await {
        warn!(
            target: "ai",
            deprecated_model = %model,
            replacement_model = %replacement,
            "[AI] Gemini model '{}' returned 404; auto-switching to '{}' and retrying",
            model,
            replacement
        );
        cache_resolved_model(&replacement).await;
        let (retry_status, retry_text) = post_generate_content(client, api_key, &replacement, body).await?;
        return Ok((retry_status, retry_text, replacement));
    }

    Ok((status, text, model))
}

pub async fn verify_gemini_api_key(client: &Client, api_key: &str) -> KeyVerificationResult {
    let body = json!({
        "contents": [{
            "parts": [{ "text": "ping" }]
        }]
    });

    let (status, err_text, model_used) = match post_generate_content_with_fallback(client, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            return KeyVerificationResult {
                status: "error".to_string(),
                message: format!("Connection error: {}", e),
                retry_delay: 0.0,
            }
        }
    };

    if status.is_success() {
        KeyVerificationResult {
            status: "healthy".to_string(),
            message: format!("Key is valid and active (model: {})", model_used),
            retry_delay: 0.0,
        }
    } else if status.as_u16() == 429 {
        KeyVerificationResult {
            status: "quota_exceeded".to_string(),
            message: "Quota exceeded for this key".to_string(),
            retry_delay: 3600.0,
        }
    } else {
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

        let started = Instant::now();
        let (status, resp_text, model_used) = match post_generate_content_with_fallback(client, &api_key, &contents).await {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    target: "ai",
                    duration_ms = started.elapsed().as_millis() as u64,
                    attempt = attempts,
                    "[AI] Request failed with key {}: {}",
                    db::mask_key(&api_key),
                    e
                );
                continue;
            }
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        if status.is_success() {
            info!(
                target: "ai",
                duration_ms = duration_ms,
                attempt = attempts,
                model = %model_used,
                "[AI] Gemini query generation succeeded in {}ms ({})",
                duration_ms,
                model_used
            );
            db::mark_gemini_key_used(pool, &api_key).await;
            let val: serde_json::Value = serde_json::from_str(&resp_text)?;
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
            warn!(
                target: "ai",
                model = %model_used,
                "[AI] Key {} error {}: {}",
                db::mask_key(&api_key),
                status,
                resp_text
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_replacement_hint_from_404_error() {
        let body = r#"{ "error": { "code": 404, "message": "This model models/gemini-2.5-flash is no longer available to new users. Please update your code to use models/gemini-3.8-flash for the latest features and improvements." } }"#;
        let hint = extract_model_hint_from_404(body, "gemini-2.5-flash");
        assert_eq!(hint.as_deref(), Some("gemini-3.8-flash"));
    }

    #[test]
    fn selects_newest_stable_flash_model_from_list() {
        let payload = json!({
            "models": [
                {
                    "name": "models/gemini-2.5-flash",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/gemini-3.8-flash-lite",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/gemini-3.8-flash",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/gemini-4.0-flash-preview",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/gemini-4.0-flash-tts",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/embedding-001",
                    "supportedGenerationMethods": ["embedContent"]
                }
            ]
        });

        // Should skip failed gemini-2.5-flash, preview, and tts; pick stable gemini-3.8-flash
        let best = select_best_gemini_model(&payload, Some("gemini-2.5-flash"), None);
        assert_eq!(best.as_deref(), Some("gemini-3.8-flash"));

        // Future deprecation: if gemini-3.8-flash ever returns 404, falls back cleanly to gemini-3.8-flash-lite
        let next = select_best_gemini_model(&payload, Some("gemini-3.8-flash"), None);
        assert_eq!(next.as_deref(), Some("gemini-3.8-flash-lite"));
    }
}
