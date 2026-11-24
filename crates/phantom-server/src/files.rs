//! Moving whole files between the browser and the workspace.
//!
//! The WebSocket carries JSON, so bytes would have to be base64'd through it —
//! wasteful for anything bigger than a screenshot, and it throws away the
//! browser's own save-file UI. These two endpoints stream instead, and sit
//! behind the same session auth as `/ws`, which already hands a logged-in
//! client arbitrary filesystem reads and writes.
//!
//! Distinct from `blob.rs`: that one drops bytes in a scratch directory to be
//! pasted into a shell and never takes a name from the client. Here the client
//! chooses the destination, so the name is validated and collisions are given a
//! suffix rather than overwriting what's already there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;
use phantom_core::workspace::{resolve_path, WorkspaceEnv};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::AppState;

/// Ceiling on one upload. Generous enough for a video or a dataset, low enough
/// that a runaway request can't fill the disk unnoticed.
const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// Highest `-N` suffix tried before giving up on finding a free name.
const MAX_COLLISIONS: u32 = 1000;

type Failure = (StatusCode, String);

#[derive(Deserialize)]
pub struct UploadQuery {
    /// Destination directory, absolute, as the client sees it.
    dir: String,
    /// Bare file name — one path component, validated below.
    name: String,
    /// WSL distribution the paths belong to, when the client browses one.
    /// Absent means the local filesystem.
    #[serde(default)]
    distro: Option<String>,
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    path: String,
    #[serde(default)]
    distro: Option<String>,
    /// Serve with the file's media type so the page can render it — the web
    /// build's stand-in for Tauri's asset protocol. Only for [`inline_type`]s.
    #[serde(default)]
    inline: bool,
}

#[derive(Serialize)]
pub struct UploadResponse {
    /// Where the bytes actually landed — not necessarily `dir/name`, since a
    /// collision gets a suffix.
    path: String,
    name: String,
}

fn workspace(distro: Option<String>) -> WorkspaceEnv {
    match distro {
        Some(distro) => WorkspaceEnv::Wsl { distro },
        None => WorkspaceEnv::Local,
    }
}

/// Accepts only a single path component. The client picks this name from a
/// file it chose, and a name like `../../.ssh/authorized_keys` would otherwise
/// write outside the directory the user pointed at.
fn safe_name(name: &str) -> Result<&str, Failure> {
    let unsafe_name = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0');
    if unsafe_name {
        return Err((StatusCode::BAD_REQUEST, format!("unsafe file name: {name:?}")));
    }
    Ok(name)
}

/// First unused path for `name` in `dir`: `notes.txt`, then `notes-1.txt`.
///
/// Never overwrites: an upload that silently replaced a file would be an
/// unrecoverable mistake made with a two-click gesture.
fn upload_path(dir: &Path, name: &str, suffix: u32) -> PathBuf {
    if suffix == 0 {
        return dir.join(name);
    }
    let as_path = Path::new(name);
    let stem = as_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = as_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    dir.join(format!("{stem}-{suffix}{ext}"))
}

async fn open_upload(dir: &Path, name: &str) -> Result<(PathBuf, tokio::fs::File), Failure> {
    for suffix in 0..=MAX_COLLISIONS {
        let path = upload_path(dir, name, suffix);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("create {}: {error}", path.display()),
                ))
            }
        }
    }
    Err((
        StatusCode::CONFLICT,
        format!("{name} and {MAX_COLLISIONS} numbered variants all exist"),
    ))
}

/// Removes the partial file before reporting the failure: half an upload
/// sitting in the tree looking like a real file is worse than none.
async fn abort(path: &Path, status: StatusCode, message: String) -> Failure {
    let _ = tokio::fs::remove_file(path).await;
    (status, message)
}

pub async fn upload(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<Json<UploadResponse>, Failure> {
    let name = safe_name(&query.name)?;
    let env = workspace(query.distro);
    let dir = resolve_path(&query.dir, &env);
    if !dir.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("not a directory: {}", dir.display()),
        ));
    }
    let _change = state.file_index.track_directory(&query.dir, Some(&env));

    // Streamed rather than buffered: `Bytes` would pull the whole upload into
    // memory, and axum's default 2 MiB extractor limit would reject anything
    // real long before that.
    let (path, mut file) = open_upload(&dir, name).await?;
    let mut stream = body.into_data_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                return Err(abort(&path, StatusCode::BAD_REQUEST, format!("read body: {e}")).await)
            }
        };
        written += chunk.len() as u64;
        if written > MAX_UPLOAD_BYTES {
            return Err(abort(
                &path,
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("upload exceeds the {MAX_UPLOAD_BYTES} byte limit"),
            )
            .await);
        }
        if let Err(e) = file.write_all(&chunk).await {
            return Err(abort(
                &path,
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write {}: {e}", path.display()),
            )
            .await);
        }
    }
    if let Err(e) = file.flush().await {
        return Err(abort(
            &path,
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("flush {}: {e}", path.display()),
        )
        .await);
    }

    log::info!("uploaded {written} bytes to {}", path.display());
    let stored = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    Ok(Json(UploadResponse {
        path: phantom_core::path::to_canon(&path),
        name: stored,
    }))
}

pub async fn download(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<DownloadQuery>,
) -> Result<Response, Failure> {
    let env = workspace(query.distro);
    let path = resolve_path(&query.path, &env);
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{}: {e}", path.display())))?;
    if meta.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            "downloading a directory is not supported".into(),
        ));
    }
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{}: {e}", path.display())))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());

    // `application/octet-stream` by default, whatever the extension: the
    // point is to save the file, and letting the browser render workspace
    // content on this origin would be a stored-XSS gift. Inline is limited to
    // media types, and sandboxed so an SVG opened on its own runs no script.
    let inline = query.inline.then(|| inline_type(&name)).flatten();
    let (content_type, disposition, csp) = match inline {
        Some(t) => (t, "inline".to_string(), if t == "application/pdf" { "" } else { "sandbox" }),
        None => ("application/octet-stream", disposition(&name), ""),
    };
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_LENGTH, meta.len().to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response();
    if !csp.is_empty() {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(csp),
        );
    }
    Ok(response)
}

/// The media types the editor previews, by extension. Anything else downloads.
fn inline_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogg" => "video/ogg",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Both halves of RFC 6266: a scrubbed ASCII `filename` every browser accepts,
/// and the UTF-8 `filename*` that carries the real name where it's understood.
fn disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || !(c.is_ascii_graphic() || c == ' ') {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        percent_encode(name)
    )
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_names_that_escape_the_destination() {
        for name in ["", ".", "..", "../evil", "a/b", r"a\b", "a\0b"] {
            assert!(safe_name(name).is_err(), "{name:?} must be rejected");
        }
        assert_eq!(safe_name("photo 1.png").unwrap(), "photo 1.png");
    }

    #[tokio::test]
    async fn suffixes_rather_than_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (path, file) = open_upload(dir.path(), "a.txt").await.unwrap();
        assert_eq!(path, dir.path().join("a.txt"));
        drop(file);

        std::fs::write(dir.path().join("a.txt"), b"first").expect("write");
        assert_eq!(
            open_upload(dir.path(), "a.txt").await.unwrap().0,
            dir.path().join("a-1.txt")
        );

        std::fs::write(dir.path().join("a-1.txt"), b"second").expect("write");
        assert_eq!(
            open_upload(dir.path(), "a.txt").await.unwrap().0,
            dir.path().join("a-2.txt")
        );

        // Extensionless names keep working.
        std::fs::write(dir.path().join("README"), b"x").expect("write");
        assert_eq!(
            open_upload(dir.path(), "README").await.unwrap().0,
            dir.path().join("README-1")
        );
    }

    #[tokio::test]
    async fn concurrent_uploads_claim_distinct_names_without_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let mut workers = Vec::new();
        for n in 0..4u8 {
            let root = dir.path().to_owned();
            workers.push(tokio::spawn(async move {
                let (path, mut file) = open_upload(&root, "same.txt").await.unwrap();
                file.write_all(&[n]).await.unwrap();
                file.flush().await.unwrap();
                (path, n)
            }));
        }
        let mut paths = std::collections::HashSet::new();
        for worker in workers {
            let (path, n) = worker.await.unwrap();
            assert!(paths.insert(path.clone()));
            assert_eq!(std::fs::read(path).unwrap(), [n]);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uploads_do_not_follow_existing_or_dangling_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("a.txt")).unwrap();
        std::os::unix::fs::symlink("missing", dir.path().join("a-1.txt")).unwrap();
        let (path, mut file) = open_upload(dir.path(), "a.txt").await.unwrap();
        file.write_all(b"new").await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(path, dir.path().join("a-2.txt"));
        assert_eq!(std::fs::read(victim).unwrap(), b"keep");
    }

    #[test]
    fn disposition_survives_a_hostile_name() {
        let header = disposition("re\"port\n漢.txt");
        assert!(!header.contains('\n'), "a raw newline would split the header");
        assert!(header.contains("filename=\"re_port_"), "{header}");
        assert!(header.contains("filename*=UTF-8''re%22port%0A%E6%BC%A2.txt"), "{header}");
    }
}
