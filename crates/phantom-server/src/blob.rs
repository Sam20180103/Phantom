//! Screenshot and file drop-off.
//!
//! Terminals can't take an image on the clipboard, so the browser posts the
//! bytes here, we write them to a scratch directory, and the client pastes the
//! resulting path into the shell — which is what an AI agent can actually read.
//!
//! Files land in `/tmp/phantom-blobs` (override with `PHANTOM_BLOB_DIR`) and
//! are pruned by age on every upload, so a long-running server doesn't slowly
//! fill the disk with pasted screenshots.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

/// Refuse anything larger; a pasted screenshot is well under this, and the cap
/// keeps a stray upload from filling the disk before the pruner sees it.
///
/// Enforced by a `DefaultBodyLimit` on the route rather than here: `Bytes`
/// buffers the whole body, so the request has to be cut off before it lands in
/// memory. Public because that layer lives in `main.rs`, where the route is.
pub const MAX_BYTES: usize = 32 * 1024 * 1024;

/// How long a dropped file stays around. Long enough to reference it from a
/// later prompt, short enough that the directory doesn't grow without bound.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Serialize)]
pub struct BlobResponse {
    /// Absolute path, ready to paste into a shell.
    path: String,
}

fn blob_dir() -> PathBuf {
    std::env::var_os("PHANTOM_BLOB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("phantom-blobs"))
}

/// Maps a content type to a file extension. Unknown types get `.bin` rather
/// than being rejected — the shell doesn't care, and guessing wrong is worse
/// than being vague.
fn extension_for(content_type: Option<&str>) -> &'static str {
    match content_type.map(|t| t.split(';').next().unwrap_or(t).trim()) {
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some("image/svg+xml") => "svg",
        Some("application/pdf") => "pdf",
        Some("text/plain") => "txt",
        _ => "bin",
    }
}

/// Deletes blobs older than [`MAX_AGE`]. Best effort: a file we can't stat or
/// remove is skipped rather than failing the upload the user is waiting on.
fn prune(dir: &PathBuf) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|modified| {
                now.duration_since(modified)
                    .map(|age| age > MAX_AGE)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub async fn upload(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<BlobResponse>, (StatusCode, String)> {
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty upload".into()));
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let dir = blob_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("create {}: {e}", dir.display())))?;
    prune(&dir);

    // Name from the clock plus randomness: the timestamp makes the directory
    // readable, the random suffix keeps two pastes in the same millisecond from
    // colliding. The client never supplies any part of the name, so a hostile
    // filename can't escape the directory.
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut suffix = [0u8; 4];
    getrandom::fill(&mut suffix).expect("system RNG unavailable");
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!(
        "{stamp}-{suffix}.{}",
        extension_for(content_type)
    ));

    std::fs::write(&path, &body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write {}: {e}", path.display())))?;

    log::info!("stored {} byte blob at {}", body.len(), path.display());
    Ok(Json(BlobResponse {
        path: phantom_core::path::to_canon(&path),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_image_types() {
        assert_eq!(extension_for(Some("image/png")), "png");
        assert_eq!(extension_for(Some("image/jpeg")), "jpg");
        // Browsers append parameters to the clipboard's content type.
        assert_eq!(extension_for(Some("image/png; charset=binary")), "png");
    }

    #[test]
    fn unknown_types_fall_back_rather_than_failing() {
        assert_eq!(extension_for(Some("application/x-thing")), "bin");
        assert_eq!(extension_for(None), "bin");
    }
}
