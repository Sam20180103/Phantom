//! Tauri command wrappers over `phantom_core::history`. Only `State` extraction
//! differs from the headless server; the shell-history parsing and PATH scan
//! live in core so both share them.

// `lib.rs` registers this state by the wrapper's path, so the import is public.
pub use phantom_core::history::HistoryState;
use phantom_core::history;

#[tauri::command]
pub fn history_suggest(state: tauri::State<'_, HistoryState>, line: String) -> Option<String> {
    history::history_suggest(&state, line)
}

#[tauri::command]
pub fn history_commands(
    state: tauri::State<'_, HistoryState>,
    prefix: String,
    limit: Option<usize>,
) -> Vec<String> {
    history::history_commands(&state, prefix, limit)
}

#[tauri::command]
pub fn history_list(
    state: tauri::State<'_, HistoryState>,
    query: String,
    limit: Option<usize>,
) -> Vec<String> {
    history::history_list(&state, query, limit)
}

#[tauri::command]
pub fn history_record(state: tauri::State<'_, HistoryState>, command: String) {
    history::history_record(&state, command)
}
