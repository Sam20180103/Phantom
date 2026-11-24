pub mod clipboard;
pub mod copy;
pub mod file;
pub mod grep;
pub mod index;
pub mod mutate;
mod publish;
pub mod search;
pub mod tree;
pub mod watch;

/// The single canonical-to-display conversion: forward slashes, Windows
/// verbatim `\\?\` prefix stripped. Route every such conversion through here.
///
/// Moved to `phantom-core` so the headless server shares one implementation;
/// re-exported here because the rest of this crate imports it from `fs`.
pub use crate::path::to_canon;

/// Where filesystem-watch notifications go.
///
/// The same seam as [`crate::pty::PtySink`]: the desktop app forwards these to
/// a Tauri event, the headless server pushes them down the WebSocket. Called
/// from the watcher thread, so implementations must not block.
pub trait FsEventSink: Send + Sync + 'static {
    /// Paths that changed since the last notification, already debounced.
    fn changed(&self, paths: Vec<String>);
}

/// Notifies the client that *we* wrote a file, so an open editor can tell its
/// own save apart from an external change and skip a needless reload.
pub trait FileEventSink: Send + Sync {
    fn file_written(&self, path: String, source: Option<String>);
}
