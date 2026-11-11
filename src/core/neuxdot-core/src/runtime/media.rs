//! Image attachments for the agent loop.
//!
//! ## Why
//!
//! Tool output is text. A plugin that returns a base64 image (or a raw PNG, or
//! an HTTPS URL to one) gets truncated like any other huge string: the model
//! sees the first few kilobytes of `iVBORw0KGgo…` and can never reason about
//! the picture.
//!
//! `imageView` is the resolution. It takes the URL / data URI / file path /
//! raw base64 *as an argument* — safe to pass because the host resolves it —
//! stores the decoded bytes once under `data/media/`, and attaches the image to
//! the model as a real `image_url` content part.
//!
//! ## Storage
//!
//! History rows hold a compact reference, never the base64:
//!
//! ```json
//! [{"type":"text","text":"Image: shot.png"},
//!  {"type":"image","path":"data/media/<sha>.png","mime":"image/png"}]
//! ```
//!
//! [`expand_content`] turns that back into an `image_url` data URL when the
//! turn is rebuilt, so the image survives restarts and history repair without
//! inflating SQLite or the transcript.
//!
//! ## Limits
//!
//! Only raster formats are accepted (PNG, JPEG, GIF, WebP). SVG is rejected:
//! it is executable content, not an image. Decoded size is capped at
//! [`MAX_IMAGE_BYTES`]; callers get a clear error instead of a provider 400.

use base64::Engine;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where decoded attachments live, relative to the server's working directory.
pub const MEDIA_DIR: &str = "data/media";

/// Largest decoded image we will attach. Bigger inputs are rejected with a
/// message telling the model to downscale first.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Organise one stored attachment.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredImage {
    /// Relative path written to history (`data/media/<sha>.<ext>`).
    pub path: String,
    pub mime: String,
    pub bytes: usize,
}

/// Identify a raster image by magic bytes. `None` means "not an image we can
/// safely hand to a provider".
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

fn decode_base64_lenient(raw: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("the base64 payload is empty".to_string());
    }
    // Accept unpadded input (common when the value was copied from a URL-safe
    // context or truncated by a previous tool result).
    let padded = match cleaned.len() % 4 {
        2 => format!("{cleaned}=="),
        3 => format!("{cleaned}="),
        _ => cleaned,
    };
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .map_err(|e| format!("invalid base64 image payload: {e}"))
}

/// Parse `data:image/png;base64,…`.
fn decode_data_url(source: &str) -> Option<Result<(String, Vec<u8>), String>> {
    let rest = source.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("").trim().to_string();
    if !meta.contains(";base64") {
        return Some(Err(
            "only base64 data URIs are supported (data:image/png;base64,…)".to_string(),
        ));
    }
    Some(decode_base64_lenient(payload).map(|bytes| (mime, bytes)))
}

/// Resolve whatever the model passed into decoded image bytes.
///
/// Accepted forms, in order:
///   * `data:image/…;base64,…`
///   * `http(s)://…`
///   * `file://…` or a filesystem path (`/tmp/shot.png`, `data/x.png`)
///   * raw base64 (optionally whitespace-wrapped)
pub async fn resolve(source: &str) -> Result<(String, Vec<u8>), String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("no image source was given".to_string());
    }

    if let Some(parsed) = decode_data_url(source) {
        let (mime, bytes) = parsed?;
        return Ok((mime, bytes));
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("cannot create the image HTTP client: {e}"))?;
        let response = client
            .get(source)
            .send()
            .await
            .map_err(|e| format!("could not download the image: {e}"))?
            .error_for_status()
            .map_err(|e| format!("image download failed: {e}"))?;
        let declared = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if let Some(len) = response.content_length() {
            if len as usize > MAX_IMAGE_BYTES {
                return Err(format!(
                    "image is {len} bytes; the limit is {MAX_IMAGE_BYTES} bytes. Downscale it first."
                ));
            }
        }
        // Stream with a hard cap: a hostile or misconfigured URL must not be
        // able to make the server buffer an unbounded body.
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("could not read the image body: {e}"))?;
            if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
                return Err(format!(
                    "image exceeds the {MAX_IMAGE_BYTES}-byte limit. Downscale it first."
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((declared, bytes));
    }

    let path = source.strip_prefix("file://").unwrap_or(source);
    if Path::new(path).exists() {
        let bytes = std::fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
        return Ok((String::new(), bytes));
    }

    // Last resort: a bare base64 blob. Long enough that it cannot be a path.
    if source.len() > 64
        && source
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=\r\n".contains(c))
    {
        return Ok((String::new(), decode_base64_lenient(source)?));
    }

    Err(format!(
        "image source is not a data URI, URL, file path, or base64 payload: {source}"
    ))
}

/// Sniff, cap and persist `bytes`, returning the reference to write to history.
pub fn store(bytes: &[u8]) -> Result<StoredImage, String> {
    if bytes.is_empty() {
        return Err("the image payload is empty".to_string());
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "image is {} bytes; the limit is {} bytes. Downscale it first and pass the smaller file.",
            bytes.len(),
            MAX_IMAGE_BYTES
        ));
    }
    let mime = sniff_mime(bytes).ok_or_else(|| {
        "unsupported image format: expected PNG, JPEG, GIF or WebP (SVG is rejected)".to_string()
    })?;

    let hash = hex::encode(Sha256::digest(bytes));
    let dir = PathBuf::from(MEDIA_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let relative = format!("{MEDIA_DIR}/{hash}.{}", extension(mime));
    let target = PathBuf::from(&relative);
    if !target.exists() {
        std::fs::write(&target, bytes)
            .map_err(|e| format!("could not write {}: {e}", target.display()))?;
    }

    Ok(StoredImage {
        path: relative,
        mime: mime.to_string(),
        bytes: bytes.len(),
    })
}

fn data_url(path: &str, mime: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{encoded}"))
}

/// Approximate context cost of a message `content` value.
///
/// A string costs its character count. A multimodal array costs its text plus
/// a nominal per-image weight: the base64 data URL is huge in bytes but the
/// provider bills an image by resolution, so counting the raw string would
/// trigger pruning on every attached picture.
pub fn content_chars(content: &Value) -> usize {
    match content {
        Value::String(s) => s.chars().count(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part.get("type").and_then(|t| t.as_str()) {
                Some("image_url") | Some("image") => 2_000,
                _ => part
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|t| t.chars().count())
                    .unwrap_or(0),
            })
            .sum(),
        _ => 0,
    }
}

/// Turn a stored history `content` value into what the provider should see.
///
/// A plain string is returned unchanged. A JSON array is parsed and every
/// `{"type":"image","path":…}` part is expanded into an `image_url` data URL.
/// A missing file degrades to a text note instead of failing the whole turn.
pub fn expand_content(content: &str) -> Value {
    let trimmed = content.trim();
    if !trimmed.starts_with('[') {
        return Value::String(content.to_string());
    }
    let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(trimmed) else {
        return Value::String(content.to_string());
    };

    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        match part.get("type").and_then(|t| t.as_str()) {
            Some("image") => {
                let path = part.get("path").and_then(|p| p.as_str()).unwrap_or("");
                let mime = part
                    .get("mime")
                    .and_then(|m| m.as_str())
                    .unwrap_or("image/png");
                let detail = part
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .unwrap_or("auto");
                match data_url(path, mime) {
                    Some(url) => out.push(json!({
                        "type": "image_url",
                        "image_url": { "url": url, "detail": detail }
                    })),
                    None => out.push(json!({
                        "type": "text",
                        "text": format!("[image unavailable: {path}]")
                    })),
                }
            }
            _ => out.push(part),
        }
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_the_supported_raster_formats_and_rejects_svg() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_mime(b"\xff\xd8\xffrest"), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89arest"), Some("image/gif"));
        assert_eq!(sniff_mime(b"RIFFxxxxWEBPrest"), Some("image/webp"));
        assert_eq!(sniff_mime(b"<svg xmlns=\"...\"></svg>"), None);
        assert_eq!(sniff_mime(b""), None);
    }

    #[test]
    fn data_url_round_trips_a_tiny_png() {
        let png = b"\x89PNG\r\n\x1a\nfake";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let (mime, bytes) = decode_data_url(&format!("data:image/png;base64,{encoded}"))
            .unwrap()
            .unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png);
    }

    #[test]
    fn unpadded_base64_is_accepted() {
        // "hi" -> aGk= (remove padding)
        let bytes = decode_base64_lenient("aGk").unwrap();
        assert_eq!(bytes, b"hi");
    }

    #[test]
    fn expand_content_replaces_an_image_reference_and_degrades_missing_files() {
        let content = r#"[{"type":"text","text":"look"},{"type":"image","path":"/nonexistent/x.png","mime":"image/png"}]"#;
        let expanded = expand_content(content);
        let parts = expanded.as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "text");
        assert!(parts[1]["text"].as_str().unwrap().contains("unavailable"));

        // A plain string is untouched.
        assert_eq!(expand_content("hello"), json!("hello"));
    }
}
