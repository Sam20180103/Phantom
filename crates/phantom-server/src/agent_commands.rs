//! Agent-hook dispatch: installs the OSC 777 notification hooks into an agent's
//! config, and reports whether they are present. Stateless — the work is
//! reading and writing the agent's own settings files.

use phantom_core::agent;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct AgentArgs {
    agent: String,
}

pub fn dispatch(cmd: &str, args: Value) -> Option<Result<Value, String>> {
    Some(match cmd {
        "agent_enable_hooks" => parse(args)
            .and_then(|a: AgentArgs| agent::agent_enable_hooks(a.agent).map(|()| Value::Null)),
        "agent_hooks_status" => {
            parse(args).map(|a: AgentArgs| serde_json::json!(agent::agent_hooks_status(a.agent)))
        }
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}
