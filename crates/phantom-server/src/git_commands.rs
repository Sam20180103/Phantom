//! Git command dispatch, the counterpart to `fs_commands` for the Source
//! Control panel.
//!
//! Every arm is a thin call into `phantom_core::git::operations`, which shells
//! out to `git` and is shared verbatim with the desktop app — the two differ
//! only in how arguments arrive. Argument names match what the frontend already
//! sends, so no call site changes.
//!
//! Synchronous like `fs_commands`: `operations` blocks on a child process, and
//! the caller has already put us on a blocking task.

use phantom_core::git::operations;
use phantom_core::git::types::DiscardEntry;
use phantom_core::workspace::WorkspaceEnv;
use serde::Deserialize;
use serde_json::Value;

use crate::session::Connection;

/// `cwd` for the two commands that locate a repository, `repo_root` for the
/// rest — the frontend distinguishes them, so this does too rather than
/// accepting either and hoping.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CwdArgs {
    cwd: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoArgs {
    repo_root: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffArgs {
    repo_root: String,
    #[serde(default)]
    path: Option<String>,
    staged: bool,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffContentArgs {
    repo_root: String,
    path: String,
    staged: bool,
    #[serde(default)]
    original_path: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathsArgs {
    repo_root: String,
    paths: Vec<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardArgs {
    repo_root: String,
    entries: Vec<DiscardEntry>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitArgs {
    repo_root: String,
    message: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogArgs {
    repo_root: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    before_sha: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShaArgs {
    repo_root: String,
    sha: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitFileDiffArgs {
    repo_root: String,
    sha: String,
    path: String,
    #[serde(default)]
    original_path: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteArgs {
    repo_root: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchArgs {
    repo_root: String,
    branch: String,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

pub fn dispatch(conn: &Connection, cmd: &str, args: Value) -> Option<Result<Value, String>> {
    let registry = conn.registry.as_ref();
    Some(match cmd {
        "git_resolve_repo" => parse::<CwdArgs>(args).and_then(|a| {
            to_value(operations::resolve_repo(
                registry,
                &a.cwd,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_panel_snapshot" => parse::<CwdArgs>(args).and_then(|a| {
            to_value(operations::panel_snapshot(
                registry,
                &a.cwd,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_status" => parse::<RepoArgs>(args).and_then(|a| {
            to_value(operations::status(
                registry,
                &a.repo_root,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_diff" => parse::<DiffArgs>(args).and_then(|a| {
            to_value(operations::diff(
                registry,
                &a.repo_root,
                a.path.as_deref(),
                a.staged,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_diff_content" => parse::<DiffContentArgs>(args).and_then(|a| {
            to_value(operations::diff_content(
                registry,
                &a.repo_root,
                &a.path,
                a.staged,
                a.original_path.as_deref(),
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_stage" => parse::<PathsArgs>(args).and_then(|a| {
            to_value(operations::stage(
                registry,
                &a.repo_root,
                &a.paths,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_unstage" => parse::<PathsArgs>(args).and_then(|a| {
            to_value(operations::unstage(
                registry,
                &a.repo_root,
                &a.paths,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_discard" => parse::<DiscardArgs>(args).and_then(|a| {
            to_value(operations::discard(
                registry,
                &a.repo_root,
                &a.entries,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_commit" => parse::<CommitArgs>(args).and_then(|a| {
            to_value(operations::commit(
                registry,
                &a.repo_root,
                &a.message,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_fetch" => parse::<RepoArgs>(args).and_then(|a| {
            to_value(operations::fetch(
                registry,
                &a.repo_root,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_pull_ff_only" => parse::<RepoArgs>(args).and_then(|a| {
            to_value(operations::pull_ff_only(
                registry,
                &a.repo_root,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_push" => parse::<RepoArgs>(args).and_then(|a| {
            to_value(operations::push(
                registry,
                &a.repo_root,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_log" => parse::<LogArgs>(args).and_then(|a| {
            to_value(operations::log(
                registry,
                &a.repo_root,
                // Same default as the desktop wrapper — the frontend omits it
                // for the initial page of history.
                a.limit.unwrap_or(30),
                a.before_sha.as_deref(),
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_show_commit" => parse::<ShaArgs>(args).and_then(|a| {
            to_value(operations::show_commit_diff(
                registry,
                &a.repo_root,
                &a.sha,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_commit_files" => parse::<ShaArgs>(args).and_then(|a| {
            to_value(operations::commit_files(
                registry,
                &a.repo_root,
                &a.sha,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_commit_file_diff" => parse::<CommitFileDiffArgs>(args).and_then(|a| {
            to_value(operations::commit_file_diff(
                registry,
                &a.repo_root,
                &a.sha,
                &a.path,
                a.original_path.as_deref(),
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_remote_url" => parse::<RemoteArgs>(args).and_then(|a| {
            to_value(operations::remote_url(
                registry,
                &a.repo_root,
                &a.name.unwrap_or_else(|| "origin".to_string()),
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_list_branches" => parse::<RepoArgs>(args).and_then(|a| {
            to_value(operations::list_branches(
                registry,
                &a.repo_root,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        "git_checkout_branch" => parse::<BranchArgs>(args).and_then(|a| {
            to_value(operations::checkout_branch(
                registry,
                &a.repo_root,
                &a.branch,
                &WorkspaceEnv::from_option(a.workspace),
            ))
        }),
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

/// `operations` returns its own error type; the wire carries strings.
fn to_value<T: serde::Serialize, E: std::fmt::Display>(
    result: Result<T, E>,
) -> Result<Value, String> {
    result
        .map_err(|e| e.to_string())
        .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}
