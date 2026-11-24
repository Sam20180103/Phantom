//! Shell command dispatch for the AI panel: one-shot runs, persistent agent
//! sessions, and background processes. All thin calls into `phantom_core::shell`
//! (which shells out and blocks), so this rides the blocking dispatch like
//! `fs_commands`.

use phantom_core::shell::{self, ShellState};
use phantom_core::workspace::WorkspaceEnv;
use serde::Deserialize;
use serde_json::Value;

use crate::session::Connection;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArgs {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenArgs {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRunArgs {
    id: u32,
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdArgs {
    id: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnArgs {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    workspace: Option<WorkspaceEnv>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsArgs {
    handle: u32,
    #[serde(default)]
    since_offset: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandleArgs {
    handle: u32,
}

pub fn dispatch(conn: &Connection, cmd: &str, args: Value) -> Option<Result<Value, String>> {
    let reg = conn.registry.as_ref();
    let st: &ShellState = &conn.shell;
    Some(match cmd {
        "shell_run_command" => parse::<RunArgs>(args).and_then(|a| {
            to_value(shell::shell_run_command(
                a.command,
                a.cwd,
                a.timeout_secs,
                a.workspace,
                reg,
            ))
        }),
        "shell_session_open" => parse::<OpenArgs>(args).and_then(|a| {
            to_value(shell::shell_session_open(st, reg, a.cwd, a.workspace))
        }),
        "shell_session_run" => parse::<SessionRunArgs>(args).and_then(|a| {
            to_value(shell::shell_session_run(
                st,
                reg,
                a.id,
                a.command,
                a.cwd,
                a.timeout_secs,
                a.workspace,
            ))
        }),
        "shell_session_close" => {
            parse::<IdArgs>(args).and_then(|a| to_value(shell::shell_session_close(st, a.id)))
        }
        "shell_bg_spawn" => parse::<SpawnArgs>(args).and_then(|a| {
            to_value(shell::shell_bg_spawn(st, reg, a.command, a.cwd, a.workspace))
        }),
        "shell_bg_logs" => parse::<LogsArgs>(args)
            .and_then(|a| to_value(shell::shell_bg_logs(st, a.handle, a.since_offset))),
        "shell_bg_kill" => {
            parse::<HandleArgs>(args).and_then(|a| to_value(shell::shell_bg_kill(st, a.handle)))
        }
        "shell_bg_list" => to_value(shell::shell_bg_list(st)),
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

fn to_value<T: serde::Serialize>(result: Result<T, String>) -> Result<Value, String> {
    result.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}
