use std::process::Stdio;
use std::time::Instant;
use regex::Regex;
use tokio::process::Command;
use tracing::{info, warn};
use crate::models::{PlaylistPreviewResponse, PlaylistTrackItem, Song};

pub fn sanitize_query(query: &str) -> String {
    let q = query.trim();
    let re = Regex::new(r#"src=["']([^"']+)["']"#).unwrap();
    if let Some(caps) = re.captures(q) {
        if let Some(matched) = caps.get(1) {
            return matched.as_str().trim().to_string();
        }
    }
    q.to_string()
}

pub fn is_youtube_link(query: &str) -> bool {
    let clean = sanitize_query(query).to_lowercase();
    let re = Regex::new(r"^(https?://)?([a-z0-9-]+\.)?(youtube\.com|youtu\.be)/").unwrap();
    re.is_match(&clean)
}

pub fn build_yt_music_search_url(query: &str) -> String {
    let cleaned = sanitize_query(query);
    format!(
        "https://music.youtube.com/search?q={}#songs",
        url::form_urlencoded::byte_serialize(cleaned.as_bytes()).collect::<String>()
    )
}

pub fn format_unavailable_reason(raw_err: &str) -> String {
    let lower = raw_err.to_lowercase();
    if lower.contains("private video") {
        "Video is private and unavailable.".to_string()
    } else if lower.contains("video unavailable")
        || lower.contains("this video is not available")
        || lower.contains("has been removed")
        || lower.contains("account associated with this video has been terminated")
    {
        "Video is unavailable or has been removed from YouTube.".to_string()
    } else if lower.contains("sign in") || lower.contains("confirm your age") || lower.contains("age-restricted") {
        "Video requires YouTube sign-in or age verification.".to_string()
    } else if lower.contains("copyright") {
        "Video is blocked due to a copyright restriction.".to_string()
    } else if lower.contains("403") || lower.contains("forbidden") || lower.contains("429") {
        "Video stream is blocked or rate-limited by YouTube (HTTP 403/429).".to_string()
    } else if lower.contains("requested format is not available") {
        "No playable audio stream is available for this video.".to_string()
    } else {
        let first_line = raw_err
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().trim_start_matches("ERROR:").trim())
            .unwrap_or("Unknown extraction error");
        format!("Video is unavailable or having issues: {}", first_line)
    }
}

pub const AUDIO_FORMAT_SPEC: &str = "ba[acodec=opus]/ba[ext=m4a]/ba/bestaudio/best";
pub const PRIMARY_PLAYER_CLIENT: &str = "android";
pub const FALLBACK_PLAYER_CLIENTS: &str = "android_vr,mweb,web";
/// Distinct client strategies tried sequentially when opening a stream yields 0 bytes (e.g. HTTP 403).
pub const PIPE_CLIENT_STRATEGIES: &[Option<&str>] = &[
    Some("android,mweb,web"),
    None, // yt-dlp built-in default client chain
    Some("tv,mweb,web"),
];

async fn run_ytdlp_dump_json(
    target: &str,
    player_client: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    let extractor_arg = format!("youtube:player_client={}", player_client);
    let output = Command::new("yt-dlp")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .args(["-f", AUDIO_FORMAT_SPEC])
        .args(["--extractor-args", &extractor_arg])
        .arg(target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    Ok(output)
}

async fn fetch_direct_metadata(
    target: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    // Use fast single-client android first (avoids android_vr 403s); fall back to android_vr,mweb,web on failure.
    let mut client_used = PRIMARY_PLAYER_CLIENT;
    let mut output = run_ytdlp_dump_json(target, client_used).await?;

    if !output.status.success() {
        let primary_err = String::from_utf8_lossy(&output.stderr);
        warn!(
            target: "youtube",
            duration_ms = started.elapsed().as_millis() as u64,
            query = %target,
            client = client_used,
            "[YouTube] {} client failed, retrying with fallback clients: {}",
            client_used,
            primary_err.trim()
        );
        client_used = FALLBACK_PLAYER_CLIENTS;
        output = run_ytdlp_dump_json(target, client_used).await?;
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        warn!(
            target: "youtube",
            duration_ms = duration_ms,
            query = %target,
            client = client_used,
            "[YouTube] yt-dlp direct metadata failed ({}ms): {}",
            duration_ms,
            err.trim()
        );
        return Err(format_unavailable_reason(&err).into());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_str)?;
    let has_stream_url = val.get("url").and_then(|v| v.as_str()).is_some();
    info!(
        target: "youtube",
        duration_ms = duration_ms,
        client = client_used,
        has_stream_url = has_stream_url,
        query = %target,
        "[YouTube] Extracted direct metadata in {}ms (client={}, cached_stream={})",
        duration_ms,
        client_used,
        has_stream_url
    );
    Ok(val)
}

async fn search_yt_music(query: &str) -> Option<String> {
    let ytm_url = build_yt_music_search_url(query);
    let fut = async {
        let output = Command::new("yt-dlp")
            .arg("--dump-single-json")
            .arg("--flat-playlist")
            .args(["--playlist-end", "5"])
            .arg("--no-warnings")
            .arg(&ytm_url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let val: serde_json::Value = serde_json::from_str(&json_str).ok()?;
        let entries = val.get("entries")?.as_array()?;

        for e in entries {
            let u = e.get("url").and_then(|v| v.as_str()).unwrap_or_default();
            if u.contains("/browse/") {
                continue;
            }
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    return Some(format!("https://www.youtube.com/watch?v={}", id));
                }
            }
            if u.contains("watch?v=") || u.contains("youtu.be/") {
                return Some(u.to_string());
            }
        }
        None
    };

    tokio::time::timeout(tokio::time::Duration::from_secs(6), fut)
        .await
        .ok()
        .flatten()
}

pub async fn extract_song_metadata(
    query: &str,
    requester_id: Option<String>,
    requester_name: Option<String>,
    requester_avatar: Option<String>,
) -> Result<Song, Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    let clean = sanitize_query(query);

    let val = if is_youtube_link(&clean) {
        fetch_direct_metadata(&clean).await?
    } else {
        // 1. Prioritize YouTube Music with '#songs' tab filter (official studio songs vs M/Vs)
        let search_started = Instant::now();
        let resolved_target = match search_yt_music(&clean).await {
            Some(url) => {
                info!(
                    target: "youtube",
                    duration_ms = search_started.elapsed().as_millis() as u64,
                    query = %clean,
                    "[YouTube] YTM search resolved in {}ms",
                    search_started.elapsed().as_millis()
                );
                url
            }
            None => {
                warn!(
                    target: "youtube",
                    duration_ms = search_started.elapsed().as_millis() as u64,
                    query = %clean,
                    "[YouTube] YTM search fallback to ytsearch1 after {}ms",
                    search_started.elapsed().as_millis()
                );
                format!("ytsearch1:{}", clean)
            }
        };

        fetch_direct_metadata(&resolved_target).await?
    };

    let title = val.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown Title").to_string();
    let total_ms = started.elapsed().as_millis() as u64;
    info!(
        target: "youtube",
        duration_ms = total_ms,
        title = %title,
        "[YouTube] Song metadata ready in {}ms: {}",
        total_ms,
        title
    );
    let artist = val.get("artist")
        .or_else(|| val.get("uploader"))
        .or_else(|| val.get("channel"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let thumbnail = val.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string());
    let duration = val.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let webpage_url = val.get("webpage_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| query.to_string());
    let stream_url = val
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let stream_user_agent = val
        .get("http_headers")
        .and_then(|h| h.get("User-Agent"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Ok(Song {
        url: webpage_url,
        title,
        artist,
        thumbnail,
        duration,
        requester_id,
        requester_handle: requester_name.clone(),
        requester_name,
        requester_avatar,
        queue_id: Some(uuid::Uuid::new_v4().to_string()),
        error: None,
        stream_url,
        stream_user_agent,
    })
}

pub const DEFAULT_PLAYLIST_PAGE_SIZE: usize = 20;

/// Parses a `yt-dlp --flat-playlist -J` payload sliced via `--playlist-items (offset+1):(offset+limit+1)`.
pub fn parse_playlist_preview_json(
    val: &serde_json::Value,
    offset: usize,
    limit: usize,
) -> PlaylistPreviewResponse {
    let page_limit = limit.clamp(1, 50);
    let title = val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("YouTube Playlist")
        .to_string();
    let uploader = val
        .get("uploader")
        .or_else(|| val.get("channel"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let playlist_thumb = val
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let entries = val
        .get("entries")
        .and_then(|v| v.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    // Requesting page_limit + 1 items lets us detect more tracks even on endless Mix (RD...) feeds
    // where playlist_count is null.
    let has_more_from_probe = entries.len() > page_limit;
    let slice_len = entries.len().min(page_limit);

    let mut tracks = Vec::with_capacity(slice_len);
    for (i, e) in entries[..slice_len].iter().enumerate() {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let e_title = e
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Track")
            .to_string();
        let artist = e
            .get("uploader")
            .or_else(|| e.get("channel"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let duration = e.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let track_url = e
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| {
                if s.starts_with("http") {
                    s.to_string()
                } else {
                    format!("https://www.youtube.com/watch?v={}", s)
                }
            })
            .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={}", id));
        let thumb = e
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id));

        tracks.push(PlaylistTrackItem {
            index: (offset + i + 1) as i32,
            id,
            title: e_title,
            artist,
            duration,
            url: track_url,
            thumbnail: thumb,
        });
    }

    let thumbnail = if playlist_thumb.is_empty() && !tracks.is_empty() {
        tracks[0].thumbnail.clone()
    } else {
        playlist_thumb
    };

    let known_count = val
        .get("playlist_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let loaded_end = offset + tracks.len();
    let has_more = has_more_from_probe || known_count.map(|c| loaded_end < c).unwrap_or(false);
    let total_tracks = known_count.map(|c| c.max(loaded_end)).unwrap_or(loaded_end);

    PlaylistPreviewResponse {
        title,
        uploader,
        thumbnail,
        total_tracks,
        offset,
        has_more,
        tracks,
    }
}

pub async fn preview_playlist(
    url: &str,
    offset: usize,
    limit: usize,
) -> Result<PlaylistPreviewResponse, Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    let clean = sanitize_query(url);
    let page_limit = limit.clamp(1, 50);
    let start_item = offset + 1;
    let fetch_end = offset + page_limit + 1;
    let range_arg = format!("{}:{}", start_item, fetch_end);

    let fut = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .args(["--playlist-items", &range_arg])
        .arg("--no-warnings")
        .arg("-J")
        .arg(&clean)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match tokio::time::timeout(tokio::time::Duration::from_secs(20), fut).await {
        Ok(res) => res?,
        Err(_) => {
            warn!(
                target: "youtube",
                duration_ms = started.elapsed().as_millis() as u64,
                url = %clean,
                range = %range_arg,
                "[YouTube] Playlist preview timed out after 20s"
            );
            return Err("Playlist preview timed out. Try pasting a public playlist or YouTube Mix URL with a video ID.".into());
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        warn!(
            target: "youtube",
            duration_ms = duration_ms,
            url = %clean,
            range = %range_arg,
            "[YouTube] Playlist preview failed ({}ms): {}",
            duration_ms,
            err.trim()
        );
        return Err(format_unavailable_reason(&err).into());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_str)?;
    let parsed = parse_playlist_preview_json(&val, offset, page_limit);

    info!(
        target: "youtube",
        duration_ms = duration_ms,
        url = %clean,
        range = %range_arg,
        returned = parsed.tracks.len(),
        has_more = parsed.has_more,
        "[YouTube] Playlist preview extracted {} tracks in {}ms (has_more={})",
        parsed.tracks.len(),
        duration_ms,
        parsed.has_more
    );

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_reason_formats_known_errors_cleanly() {
        assert_eq!(
            format_unavailable_reason("ERROR: [youtube] xyz: Video unavailable"),
            "Video is unavailable or has been removed from YouTube."
        );
        assert_eq!(
            format_unavailable_reason("ERROR: Private video. Sign in if you've been granted access"),
            "Video is private and unavailable."
        );
        assert_eq!(
            format_unavailable_reason("ERROR: unable to download video data: HTTP Error 403: Forbidden"),
            "Video stream is blocked or rate-limited by YouTube (HTTP 403/429)."
        );
    }

    #[test]
    fn parses_paginated_playlist_preview_with_probe_item() {
        let entries: Vec<serde_json::Value> = (1..=21)
            .map(|i| serde_json::json!({ "id": format!("vid{}", i), "title": format!("Song {}", i), "duration": 120.0 }))
            .collect();
        let payload = serde_json::json!({
            "title": "Mix - Test",
            "entries": entries
        });

        let page1 = parse_playlist_preview_json(&payload, 0, 20);
        assert_eq!(page1.tracks.len(), 20);
        assert!(page1.has_more);
        assert_eq!(page1.tracks[0].index, 1);
        assert_eq!(page1.tracks[19].index, 20);

        let page2_entries: Vec<serde_json::Value> = (21..=25)
            .map(|i| serde_json::json!({ "id": format!("vid{}", i), "title": format!("Song {}", i), "duration": 100.0 }))
            .collect();
        let payload2 = serde_json::json!({
            "title": "Mix - Test",
            "playlist_count": 25,
            "entries": page2_entries
        });
        let page2 = parse_playlist_preview_json(&payload2, 20, 20);
        assert_eq!(page2.tracks.len(), 5);
        assert!(!page2.has_more);
        assert_eq!(page2.tracks[0].index, 21);
        assert_eq!(page2.tracks[4].index, 25);
        assert_eq!(page2.total_tracks, 25);
    }
}
