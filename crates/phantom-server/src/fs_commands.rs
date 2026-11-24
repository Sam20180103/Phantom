//! Filesystem command dispatch, split out so `session.rs` stays about the
//! connection rather than growing an ever-longer match arm.

use std::sync::Arc;

use phantom_core::fs;
use phantom_core::workspace::WorkspaceEnv;
use serde::Deserialize;
use serde_json::Value;

use crate::session::Connection;

// Every argument struct below is `rename_all = "camelCase"` because the
// frontend was written against Tauri, which converts camelCase argument names
// to snake_case on the way in. Nothing does that here, so `showHidden` would
// otherwise miss `show_hidden` — silently, for any field with `#[serde(default)]`,
// which is how "show hidden files" and the git decorations reached the tree as
// `false` no matter what the user picked.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathArgs {
    path: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadDirArgs {
    path: String,
    #[serde(default)]
    show_hidden: bool,
    #[serde(default)]
    git_decorations: Option<bool>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubdirsArgs {
    path: String,
    #[serde(default)]
    show_hidden: bool,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    allow_binary: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyArgs {
    sources: Vec<String>,
    #[serde(alias = "to")]
    dest_dir: String,
    root: String,
    #[serde(default)]
    rename_on_conflict: bool,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FromToArgs {
    from: String,
    to: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchArgs {
    paths: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

/// Returns `None` when `cmd` isn't a filesystem command, so the caller can go
/// on looking rather than treating it as an error.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrepArgs {
    pattern: String,
    root: String,
    #[serde(default)]
    glob: Option<Vec<String>>,
    #[serde(default)]
    case_insensitive: Option<bool>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrepInteractiveArgs {
    pattern: String,
    root: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobArgs {
    pattern: String,
    root: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchArgs {
    root: String,
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    show_hidden: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListFilesArgs {
    root: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    max_depth: Option<usize>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    show_hidden: Option<bool>,
}

pub fn dispatch(conn: &Connection, cmd: &str, args: Value) -> Option<Result<Value, String>> {
    Some(match cmd {
        "fs_read_dir" => parse::<ReadDirArgs>(args).and_then(|a| {
            to_value(fs::tree::fs_read_dir(
                a.path,
                a.show_hidden,
                a.git_decorations,
                a.workspace,
            ))
        }),
        "list_subdirs" => parse::<SubdirsArgs>(args)
            .and_then(|a| to_value(fs::tree::list_subdirs(a.path, a.show_hidden, a.workspace))),
        "fs_read_file" => parse::<ReadFileArgs>(args).and_then(|a| {
            to_value(block_on(fs::file::fs_read_file(
                a.path,
                a.workspace,
                a.allow_binary,
            )))
        }),
        "fs_write_file" => parse::<WriteArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_entries(&[&a.path], a.workspace.as_ref());
            to_value(block_on(fs::file::fs_write_file(
                a.path,
                a.content,
                a.workspace,
                a.source,
                conn.as_ref(),
            )))
        }),
        "fs_stat" => parse::<PathArgs>(args)
            .and_then(|a| to_value(block_on(fs::file::fs_stat(a.path, a.workspace)))),
        "fs_canonicalize" => parse::<PathArgs>(args)
            .and_then(|a| to_value(block_on(fs::file::fs_canonicalize(a.path, a.workspace)))),
        "fs_create_file" => parse::<PathArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_entries(&[&a.path], a.workspace.as_ref());
            to_value(fs::mutate::fs_create_file(a.path, a.workspace))
        }),
        "fs_create_dir" => parse::<PathArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_entries(&[&a.path], a.workspace.as_ref());
            to_value(fs::mutate::fs_create_dir(a.path, a.workspace))
        }),
        "fs_delete" => parse::<PathArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_entries(&[&a.path], a.workspace.as_ref());
            to_value(fs::mutate::fs_delete(a.path, a.workspace))
        }),
        "fs_rename" => parse::<FromToArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_entries(&[&a.from, &a.to], a.workspace.as_ref());
            to_value(fs::mutate::fs_rename(a.from, a.to, a.workspace))
        }),
        "fs_copy" => parse::<CopyArgs>(args).and_then(|a| {
            let _change = conn
                .app
                .file_index
                .track_directory(&a.dest_dir, a.workspace.as_ref());
            to_value(fs::copy::fs_copy(
                a.sources,
                a.dest_dir,
                a.root,
                a.workspace,
                &conn.registry,
                a.rename_on_conflict,
            ))
        }),
        "fs_watch_add" => parse::<WatchArgs>(args).and_then(|a| {
            let _resync: Vec<_> = a
                .paths
                .iter()
                .map(|path| {
                    conn.app
                        .file_index
                        .track_directory(path, a.workspace.as_ref())
                })
                .collect();
            let sink: Arc<dyn fs::FsEventSink> = conn.clone_as_fs_sink();
            to_value(fs::watch::fs_watch_add(
                a.paths,
                a.files,
                a.workspace,
                &sink,
                &conn.fs_watch,
                &conn.registry,
            ))
        }),
        "fs_watch_remove" => parse::<WatchArgs>(args).and_then(|a| {
            to_value(fs::watch::fs_watch_remove(
                a.paths,
                a.files,
                a.workspace,
                &conn.fs_watch,
            ))
        }),
        "fs_search" => parse::<SearchArgs>(args).and_then(|a| {
            to_value(fs::search::fs_search(
                &conn.app.file_index,
                a.root,
                a.query,
                a.limit,
                a.workspace,
                a.show_hidden,
            ))
        }),
        "fs_list_files" => parse::<ListFilesArgs>(args).and_then(|a| {
            to_value(fs::search::fs_list_files(
                &conn.app.file_index,
                a.root,
                a.limit,
                a.max_depth,
                a.workspace,
                a.show_hidden,
            ))
        }),
        "fs_grep" => parse::<GrepArgs>(args).and_then(|a| {
            to_value(fs::grep::fs_grep(
                a.pattern,
                a.root,
                a.glob,
                a.case_insensitive,
                a.max_results,
                a.workspace,
            ))
        }),
        // Takes the connection's generation counter, so typing a new query
        // abandons the search still walking the tree for the previous one.
        "fs_grep_interactive" => parse::<GrepInteractiveArgs>(args).and_then(|a| {
            to_value(fs::grep::fs_grep_interactive(
                &conn.content_search,
                a.pattern,
                a.root,
                a.max_results,
                a.workspace,
            ))
        }),
        "fs_glob" => parse::<GlobArgs>(args).and_then(|a| {
            to_value(fs::grep::fs_glob(
                a.pattern,
                a.root,
                a.max_results,
                a.workspace,
            ))
        }),
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

fn to_value<T: serde::Serialize>(result: Result<T, String>) -> Result<Value, String> {
    result.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}

/// These `async fn`s never await anything — they were declared async only to
/// keep Tauri's command worker free. We're already on a blocking task here, so
/// resolving them inline is correct and avoids dragging in a runtime handle.
fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut fut = Box::pin(fut);
    match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => unreachable!("phantom-core fs futures never suspend"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_reads_dest_dir_root_and_conflict_policy() {
        let args: CopyArgs = parse(serde_json::json!({
            "sources": ["/a"], "destDir": "/root/dest", "root": "/root", "renameOnConflict": true
        }))
        .unwrap();
        assert_eq!(args.dest_dir, "/root/dest");
        assert_eq!(args.root, "/root");
        assert!(args.rename_on_conflict);
        let legacy: CopyArgs =
            parse(serde_json::json!({ "sources": [], "to": "/root", "root": "/root" })).unwrap();
        assert_eq!(legacy.dest_dir, "/root");
        assert!(!legacy.rename_on_conflict);
    }

    #[test]
    fn copy_requires_a_captured_root_and_unambiguous_destination() {
        assert!(
            parse::<CopyArgs>(serde_json::json!({ "sources": [], "destDir": "/root" })).is_err()
        );
        assert!(parse::<CopyArgs>(serde_json::json!({ "sources": [], "destDir": "/root", "to": "/other", "root": "/root" })).is_err());
    }

    /// The frontend was written against Tauri, which converts camelCase
    /// argument names to snake_case. Nothing does that here, so these structs
    /// carry `rename_all` — and without it a `#[serde(default)]` field goes
    /// quietly to its default instead of failing, which is how "show hidden
    /// files" reached the tree as `false` no matter what the user picked.
    #[test]
    fn reads_arguments_the_way_the_frontend_spells_them() {
        let args = serde_json::json!({
            "path": "/tmp",
            "showHidden": true,
            "gitDecorations": true,
        });
        let parsed: ReadDirArgs = parse(args).expect("camelCase arguments parse");
        assert!(
            parsed.show_hidden,
            "showHidden must not be silently dropped"
        );
        assert_eq!(parsed.git_decorations, Some(true));
    }

    /// Guards the direction of the bug: snake_case must *not* also work, or a
    /// call site could drift back and nothing would catch it.
    #[test]
    fn ignores_snake_case_argument_names() {
        let args = serde_json::json!({ "path": "/tmp", "show_hidden": true });
        let parsed: ReadDirArgs = parse(args).expect("unknown fields are tolerated");
        assert!(
            !parsed.show_hidden,
            "snake_case is not what the frontend sends; accepting it would hide drift"
        );
    }

    #[test]
    fn grep_reads_its_camel_case_arguments() {
        let args = serde_json::json!({
            "pattern": "needle",
            "root": "/tmp",
            "caseInsensitive": true,
            "maxResults": 7,
        });
        let parsed: GrepArgs = parse(args).expect("camelCase arguments parse");
        assert_eq!(parsed.case_insensitive, Some(true));
        assert_eq!(parsed.max_results, Some(7));
    }
}
