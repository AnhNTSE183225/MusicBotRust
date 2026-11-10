use std::process::Stdio;
use regex::Regex;
use tokio::process::Command;
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

async fn fetch_direct_metadata(
    target: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let output = Command::new("yt-dlp")
        .arg("--dump-json")
        .arg("--no-playlist")
        .args(["--extractor-args", "youtube:player_client=android,ios,mweb,web"])
        .arg(target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp metadata extraction failed: {}", err).into());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_str)?;
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
    let clean = sanitize_query(query);

    let val = if is_youtube_link(&clean) {
        fetch_direct_metadata(&clean).await?
    } else {
        // 1. Prioritize YouTube Music with '#songs' tab filter (official studio songs vs M/Vs)
        let resolved_target = match search_yt_music(&clean).await {
            Some(url) => url,
            None => format!("ytsearch1:{}", clean),
        };

        fetch_direct_metadata(&resolved_target).await?
    };

    let title = val.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown Title").to_string();
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
    })
}

pub async fn preview_playlist(url: &str) -> Result<PlaylistPreviewResponse, Box<dyn std::error::Error + Send + Sync>> {
    let clean = sanitize_query(url);

    let output = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("-J")
        .arg(&clean)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp playlist extraction failed: {}", err).into());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_str)?;

    let title = val.get("title").and_then(|v| v.as_str()).unwrap_or("YouTube Playlist").to_string();
    let uploader = val.get("uploader").or_else(|| val.get("channel")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let playlist_thumb = val.get("thumbnail").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let mut tracks = Vec::new();
    if let Some(entries) = val.get("entries").and_then(|v| v.as_array()) {
        for (i, e) in entries.iter().enumerate() {
            let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let e_title = e.get("title").and_then(|v| v.as_str()).unwrap_or("Track").to_string();
            let artist = e.get("uploader").or_else(|| e.get("channel")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let duration = e.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let track_url = e.get("url")
                .and_then(|v| v.as_str())
                .map(|s| if s.starts_with("http") { s.to_string() } else { format!("https://www.youtube.com/watch?v={}", s) })
                .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={}", id));
            let thumb = e.get("thumbnail").and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id));

            tracks.push(PlaylistTrackItem {
                index: (i + 1) as i32,
                id,
                title: e_title,
                artist,
                duration,
                url: track_url,
                thumbnail: thumb,
            });
        }
    }

    let thumbnail = if playlist_thumb.is_empty() && !tracks.is_empty() {
        tracks[0].thumbnail.clone()
    } else {
        playlist_thumb
    };

    let total = tracks.len();
    Ok(PlaylistPreviewResponse {
        title,
        uploader,
        thumbnail,
        total_tracks: total,
        tracks,
    })
}
