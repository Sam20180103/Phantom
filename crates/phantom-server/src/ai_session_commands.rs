use phantom_core::ai_sessions;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Args {
    #[serde(default)]
    id: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    migrate: bool,
    #[serde(default)]
    durable: bool,
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    max_age_secs: u64,
}

pub fn dispatch(cmd: &str, args: Value) -> Option<Result<Value, String>> {
    if !matches!(
        cmd,
        "ai_session_load"
            | "ai_session_save"
            | "ai_session_delete"
            | "ai_session_finish_migration"
            | "ai_session_prune_backup"
    ) {
        return None;
    }
    Some((|| {
        let a: Args = serde_json::from_value(args).map_err(|e| e.to_string())?;
        let root = dirs::data_dir()
            .ok_or("application data directory unavailable")?
            .join("phantom");
        match cmd {
            "ai_session_load" => ai_sessions::load(&root, &a.id).map(|s| serde_json::json!(s)),
            "ai_session_save" => ai_sessions::save(&root, &a.id, &a.content, a.migrate, a.durable)
                .map(|_| Value::Null),
            "ai_session_delete" => ai_sessions::delete(&root, &a.id).map(|_| Value::Null),
            "ai_session_finish_migration" => ai_sessions::finish(&root, &a.ids).map(Value::Bool),
            "ai_session_prune_backup" => ai_sessions::prune(&root, a.max_age_secs).map(Value::Bool),
            _ => unreachable!(),
        }
    })())
}
