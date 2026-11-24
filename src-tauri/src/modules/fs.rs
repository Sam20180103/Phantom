//! Tauri command surface for the filesystem layer.
//!
//! The logic lives in `phantom_core::fs`; this file only adapts it to Tauri —
//! `State` extraction, and sinks that turn core's callbacks into webview events.
//! The headless server implements the same two sinks over a WebSocket, which is
//! why both ends emit the same event names and the frontend needs no branch.
//!
//! Grouped into submodules mirroring core's, so a command's path says which
//! part of the filesystem surface it belongs to.

use std::sync::Arc;

use phantom_core::fs;
use tauri::{AppHandle, Emitter, Manager};
pub mod clipboard;
pub use phantom_core::fs::index::FileIndexState;

/// Path canonicalisation used by `lib.rs` and the workspace module. It lives in
/// core's `path` module now; re-exported here because both call sites reach it
/// through this one.
pub use phantom_core::path::to_canon;

/// Debounced paths that changed underneath us.
const CHANGED_EVENT: &str = "fs:changed";
/// A write *we* performed, so an open editor can tell its own save apart from
/// an external change and skip a needless reload.
const FILE_WRITTEN_EVENT: &str = "fs:file-written";

/// Bridges core's filesystem callbacks to the webview.
struct AppSink {
    app: AppHandle,
}

impl fs::FsEventSink for AppSink {
    fn changed(&self, paths: Vec<String>) {
        for path in &paths {
            self.app.state::<FileIndexState>().note_change(path);
        }
        // A failed emit means the window is gone; the watcher is torn down
        // separately and there is nothing useful to do here.
        let _ = self
            .app
            .emit(CHANGED_EVENT, serde_json::json!({ "paths": paths }));
    }
}

impl fs::FileEventSink for AppSink {
    fn file_written(&self, path: String, source: Option<String>) {
        let _ = self.app.emit(
            FILE_WRITTEN_EVENT,
            serde_json::json!({ "path": path, "source": source }),
        );
    }
}

pub mod tree {
    use super::*;
    use phantom_core::fs::tree::DirEntry;
    use phantom_core::workspace::WorkspaceEnv;

    #[tauri::command]
    pub fn fs_read_dir(
        path: String,
        show_hidden: bool,
        git_decorations: Option<bool>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<Vec<DirEntry>, String> {
        fs::tree::fs_read_dir(path, show_hidden, git_decorations, workspace)
    }

    #[tauri::command]
    pub fn list_subdirs(
        path: String,
        show_hidden: bool,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<Vec<String>, String> {
        fs::tree::list_subdirs(path, show_hidden, workspace)
    }
}

pub mod file {
    use super::*;
    use phantom_core::fs::file::{FileStat, ReadResult};
    use phantom_core::workspace::WorkspaceEnv;

    #[tauri::command]
    pub async fn fs_read_file(
        path: String,
        workspace: Option<WorkspaceEnv>,
        force: Option<bool>,
    ) -> Result<ReadResult, String> {
        fs::file::fs_read_file(path, workspace, force).await
    }

    #[tauri::command]
    pub async fn fs_write_file(
        app: AppHandle,
        index: tauri::State<'_, FileIndexState>,
        path: String,
        content: String,
        workspace: Option<WorkspaceEnv>,
        source: Option<String>,
    ) -> Result<u64, String> {
        let sink = AppSink { app };
        let _change = index.track_entries(&[&path], workspace.as_ref());
        fs::file::fs_write_file(path, content, workspace, source, &sink).await
    }

    #[tauri::command]
    pub async fn fs_canonicalize(
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<String, String> {
        fs::file::fs_canonicalize(path, workspace).await
    }

    #[tauri::command]
    pub async fn fs_stat(
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<FileStat, String> {
        fs::file::fs_stat(path, workspace).await
    }
}

pub mod mutate {
    use super::*;
    use phantom_core::workspace::WorkspaceEnv;

    #[tauri::command]
    pub fn fs_create_file(
        index: tauri::State<'_, FileIndexState>,
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _change = index.track_entries(&[&path], workspace.as_ref());
        fs::mutate::fs_create_file(path, workspace)
    }

    #[tauri::command]
    pub fn fs_create_dir(
        index: tauri::State<'_, FileIndexState>,
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _change = index.track_entries(&[&path], workspace.as_ref());
        fs::mutate::fs_create_dir(path, workspace)
    }

    #[tauri::command]
    pub fn fs_rename(
        index: tauri::State<'_, FileIndexState>,
        from: String,
        to: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _change = index.track_entries(&[&from, &to], workspace.as_ref());
        fs::mutate::fs_rename(from, to, workspace)
    }

    #[tauri::command]
    pub fn fs_delete(
        index: tauri::State<'_, FileIndexState>,
        path: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        let _change = index.track_entries(&[&path], workspace.as_ref());
        fs::mutate::fs_delete(path, workspace)
    }

    #[tauri::command]
    pub async fn fs_copy(
        app: AppHandle,
        sources: Vec<String>,
        dest_dir: String,
        root: String,
        workspace: Option<WorkspaceEnv>,
        rename_on_conflict: Option<bool>,
    ) -> Result<fs::copy::CopyResult, String> {
        use phantom_core::workspace::WorkspaceRegistry;
        use tauri::Manager;
        tauri::async_runtime::spawn_blocking(move || {
            let _change = app
                .state::<FileIndexState>()
                .track_directory(&dest_dir, workspace.as_ref());
            fs::copy::fs_copy(
                sources,
                dest_dir,
                root,
                workspace,
                &app.state::<WorkspaceRegistry>(),
                rename_on_conflict.unwrap_or(false),
            )
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

pub mod watch {
    use super::*;
    pub use phantom_core::fs::watch::FsWatchState;
    use phantom_core::workspace::{WorkspaceEnv, WorkspaceRegistry};
    use tauri::Manager;

    #[tauri::command]
    pub async fn fs_watch_add(
        app: AppHandle,
        paths: Vec<String>,
        files: Option<Vec<String>>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(move || {
            let sink: Arc<dyn fs::FsEventSink> = Arc::new(AppSink { app: app.clone() });
            let _resync: Vec<_> = paths
                .iter()
                .map(|path| {
                    app.state::<FileIndexState>()
                        .track_directory(path, workspace.as_ref())
                })
                .collect();
            fs::watch::fs_watch_add(
                paths,
                files.unwrap_or_default(),
                workspace,
                &sink,
                &app.state::<FsWatchState>(),
                &app.state::<WorkspaceRegistry>(),
            )
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    pub async fn fs_watch_remove(
        app: AppHandle,
        paths: Vec<String>,
        files: Option<Vec<String>>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(move || {
            fs::watch::fs_watch_remove(
                paths,
                files.unwrap_or_default(),
                workspace,
                &app.state::<FsWatchState>(),
            )
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

pub mod search {
    use super::*;
    use phantom_core::fs::search::{ListFilesResult, SearchResult};
    use phantom_core::workspace::WorkspaceEnv;

    #[tauri::command]
    pub async fn fs_search(
        index: tauri::State<'_, FileIndexState>,
        root: String,
        query: String,
        limit: Option<usize>,
        workspace: Option<WorkspaceEnv>,
        show_hidden: Option<bool>,
    ) -> Result<SearchResult, String> {
        let index = index.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            fs::search::fs_search(&index, root, query, limit, workspace, show_hidden)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[tauri::command]
    pub async fn fs_list_files(
        index: tauri::State<'_, FileIndexState>,
        root: String,
        limit: Option<usize>,
        max_depth: Option<usize>,
        workspace: Option<WorkspaceEnv>,
        show_hidden: Option<bool>,
    ) -> Result<ListFilesResult, String> {
        let index = index.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            fs::search::fs_list_files(&index, root, limit, max_depth, workspace, show_hidden)
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

pub mod grep {
    use super::*;
    use phantom_core::fs::grep::{GlobResponse, GrepResponse};
    use phantom_core::workspace::WorkspaceEnv;

    pub use phantom_core::fs::grep::ContentSearchState;

    #[tauri::command]
    #[allow(clippy::too_many_arguments)]
    pub fn fs_grep(
        pattern: String,
        root: String,
        glob: Option<Vec<String>>,
        case_insensitive: Option<bool>,
        max_results: Option<usize>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<GrepResponse, String> {
        fs::grep::fs_grep(
            pattern,
            root,
            glob,
            case_insensitive,
            max_results,
            workspace,
        )
    }

    /// Interactive search shares one generation counter, so a newer keystroke
    /// cancels the query still running for the previous one.
    #[tauri::command]
    pub fn fs_grep_interactive(
        state: tauri::State<'_, ContentSearchState>,
        pattern: String,
        root: String,
        max_results: Option<usize>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<GrepResponse, String> {
        fs::grep::fs_grep_interactive(&state, pattern, root, max_results, workspace)
    }

    #[tauri::command]
    pub fn fs_glob(
        pattern: String,
        root: String,
        max_results: Option<usize>,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<GlobResponse, String> {
        fs::grep::fs_glob(pattern, root, max_results, workspace)
    }
}
