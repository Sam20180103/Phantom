//! Command-history dispatch: suggestions and completions for the terminal
//! block input. Backed by a shared index (shell history files + PATH), so it
//! lives on `AppState` rather than per-connection — the machine's history is
//! the same for every tab, and building it reads files and scans PATH.

use phantom_core::history::{self, HistoryState};
use serde::Deserialize;
use serde_json::Value;

use crate::session::Connection;

#[derive(Deserialize)]
struct LineArgs {
    line: String,
}

#[derive(Deserialize)]
struct CommandsArgs {
    prefix: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ListArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct RecordArgs {
    command: String,
}

pub fn dispatch(conn: &Connection, cmd: &str, args: Value) -> Option<Result<Value, String>> {
    let st: &HistoryState = &conn.history;
    Some(match cmd {
        "history_suggest" => parse::<LineArgs>(args)
            .map(|a| serde_json::json!(history::history_suggest(st, a.line))),
        "history_commands" => parse::<CommandsArgs>(args)
            .map(|a| serde_json::json!(history::history_commands(st, a.prefix, a.limit))),
        "history_list" => parse::<ListArgs>(args)
            .map(|a| serde_json::json!(history::history_list(st, a.query, a.limit))),
        "history_record" => parse::<RecordArgs>(args).map(|a| {
            history::history_record(st, a.command);
            Value::Null
        }),
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}
