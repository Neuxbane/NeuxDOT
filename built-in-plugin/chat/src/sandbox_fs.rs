//! Sandboxed file storage and execution for the chat plugin.
//!
//! The model needs to be able to hand the *user* a file and to run a command
//! against one. Both happen inside the plugin's own directory tree
//! (`data/chat/agents/<agent>/`), which the host mounts read-write into the
//! plugin's bubblewrap sandbox and nowhere else. The plugin never receives a
//! host path: every argument is resolved against that root and any escape
//! attempt is refused before a syscall happens.
//!
//! Execution uses the same isolation as any other plugin process: no network,
//! read-only `/usr`, private `/tmp`, and the agent workspace as the only
//! writable mount.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Cap on a single uploaded file. Large enough for a document or an image,
/// small enough that a model cannot fill the disk by accident.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Cap on captured stdout/stderr from one command, so a runaway command cannot
/// exhaust the context window.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Default execution timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Hard ceiling on the requested timeout.
pub const MAX_TIMEOUT_SECS: u64 = 300;

/// Resolve `requested` inside `root`, refusing anything that escapes it.
///
/// Accepts `notes/todo.md`, `/workspace/notes/todo.md` and `./notes/todo.md`
/// (the model sees the sandbox mount point, not the host path). Refuses `..`,
/// absolute paths outside the mount, and NUL bytes.
pub fn resolve_in_root(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let raw = requested.trim();
    if raw.is_empty() {
        return Err("'path' is required".to_string());
    }
    if raw.contains('\0') {
        return Err("'path' contains a NUL byte".to_string());
    }

    // The model addresses the sandbox mount; strip it so relative resolution
    // starts at the agent root.
    let relative = raw
        .strip_prefix("/workspace/")
        .or_else(|| if raw == "/workspace" { Some("") } else { None })
        .unwrap_or(raw);

    if Path::new(relative).is_absolute() {
        return Err(format!(
            "'{requested}' is an absolute host path. Files live under /workspace; pass a path relative to it."
        ));
    }

    let mut out = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "'{requested}' climbs out of the workspace with '..', which is refused"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("'{requested}' is not a relative path"));
            }
        }
    }
    Ok(out)
}

/// Decode standard base64 (with or without padding, `+`/`-` and `/`/`_` both
/// accepted) into bytes.
///
/// Hand-written so the plugin keeps its zero-dependency lockfile; a decoder for
/// a fixed alphabet is small enough that a dependency would cost more than it
/// saves.
pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let cleaned: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if cleaned.len() % 4 == 1 {
        return Err("base64 length is invalid".to_string());
    }

    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in cleaned {
        if byte == b'=' {
            break;
        }
        let Some(v) = value(byte) else {
            return Err(format!(
                "invalid base64 character {:?}",
                char::from(byte)
            ));
        };
        buffer = (buffer << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

/// Encode bytes as standard base64 with padding.
pub fn encode_base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map(|b| u32::from(*b)).unwrap_or(0);
        let b2 = chunk.get(2).map(|b| u32::from(*b)).unwrap_or(0);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(ALPHABET[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHABET[((triple >> 12) & 0x3f) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[((triple >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[(triple & 0x3f) as usize])
        } else {
            '='
        });
    }
    out
}

/// Truncate captured output to [`MAX_OUTPUT_BYTES`], on a char boundary, and
/// say so explicitly.
///
/// A silently cut transcript is the failure mode that makes an agent distrust a
/// correct command; the marker carries the byte counts so the model can decide
/// whether to re-run with a narrower command.
pub fn clamp_output(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw).to_string();
    if raw.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n<<truncated: {} of {} bytes kept>>",
        &text[..end],
        end,
        raw.len()
    )
}

/// Clamp a requested timeout into `[1, MAX_TIMEOUT_SECS]`.
pub fn clamp_timeout(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS)
}

/// Convenience wrapper so callers cannot forget the Duration.
pub fn timeout_duration(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/data/chat/agents/agent-1")
    }

    #[test]
    fn accepts_relative_and_mount_prefixed_paths() {
        assert_eq!(
            resolve_in_root(&root(), "notes/todo.md").unwrap(),
            root().join("notes/todo.md")
        );
        assert_eq!(
            resolve_in_root(&root(), "/workspace/notes/todo.md").unwrap(),
            root().join("notes/todo.md")
        );
        assert_eq!(
            resolve_in_root(&root(), "./a.txt").unwrap(),
            root().join("a.txt")
        );
    }

    #[test]
    fn refuses_traversal_and_host_absolute_paths() {
        assert!(resolve_in_root(&root(), "../../etc/passwd").is_err());
        assert!(resolve_in_root(&root(), "a/../../b").is_err());
        assert!(resolve_in_root(&root(), "/etc/passwd").is_err());
        assert!(resolve_in_root(&root(), "").is_err());
        assert!(resolve_in_root(&root(), "a\0b").is_err());
    }

    #[test]
    fn base64_round_trips() {
        for case in ["", "a", "ab", "abc", "hello world", "\u{1f600} emoji"] {
            let encoded = encode_base64(case.as_bytes());
            assert_eq!(
                decode_base64(&encoded).unwrap(),
                case.as_bytes(),
                "round trip failed for {case:?}"
            );
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        // Unpadded and url-safe alphabets are accepted.
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(decode_base64("-_8=").unwrap(), decode_base64("+/8=").unwrap());
        assert!(decode_base64("!!!!").is_err());
    }

    #[test]
    fn output_clamp_is_self_describing() {
        let small = b"ok";
        assert_eq!(clamp_output(small), "ok");

        let big = vec![b'x'; MAX_OUTPUT_BYTES + 100];
        let clamped = clamp_output(&big);
        assert!(clamped.contains("<<truncated:"));
        assert!(clamped.contains(&big.len().to_string()));
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(0)), 1);
        assert_eq!(clamp_timeout(Some(9999)), MAX_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(12)), 12);
    }
}
