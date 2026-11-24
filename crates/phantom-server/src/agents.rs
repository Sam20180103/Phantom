//! Multi-CLI orchestration config: `.phantom/agents.yaml`, which names the
//! agents a workspace collaborates with (claude=backend, agy=frontend, …).
//!
//! Step one of the orchestration design (see `Phantom-任务清单.md` §二 P1): the
//! config plus named sessions. Later steps add `dispatch`/`nudge` and a
//! pluggable orchestrator, all keyed on the agent names defined here.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One agent as declared in the config, plus whether it currently has a live
/// session. `running` is filled in by the caller from the rmux session list.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDef {
    pub name: String,
    pub cmd: String,
    pub role: String,
    pub running: bool,
}

#[derive(Deserialize)]
struct RawAgent {
    cmd: String,
    #[serde(default)]
    role: String,
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    agents: std::collections::BTreeMap<String, RawAgent>,
}

/// Reads `<root>/.phantom/agents.yaml`. A missing file is not an error — it
/// means the workspace hasn't opted into orchestration — so it returns an empty
/// list rather than failing the command.
pub fn load(root: &Path) -> Result<Vec<AgentDef>, String> {
    let path = root.join(".phantom/agents.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let cfg: RawConfig =
        serde_yaml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    // BTreeMap keeps the order stable across calls, so the UI list doesn't
    // reshuffle between refreshes.
    Ok(cfg
        .agents
        .into_iter()
        .map(|(name, a)| AgentDef {
            name,
            cmd: a.cmd,
            role: a.role,
            running: false,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_not_error() {
        // A workspace that hasn't opted into orchestration must not fail the
        // command — the panel just shows no agents.
        let dir = std::env::temp_dir().join(format!("phantom-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).unwrap().len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_names_cmds_and_roles() {
        let dir = std::env::temp_dir().join(format!("phantom-agents-p-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".phantom")).unwrap();
        std::fs::write(
            dir.join(".phantom/agents.yaml"),
            "agents:\n  claude:\n    cmd: claude\n    role: backend\n  agy:\n    cmd: antigravity\n",
        )
        .unwrap();
        let agents = load(&dir).unwrap();
        assert_eq!(agents.len(), 2);
        // BTreeMap order: agy before claude.
        assert_eq!(agents[0].name, "agy");
        assert_eq!(agents[0].cmd, "antigravity");
        assert_eq!(agents[0].role, ""); // role defaults empty
        assert_eq!(agents[1].name, "claude");
        assert_eq!(agents[1].role, "backend");
        assert!(agents.iter().all(|a| !a.running));
        std::fs::remove_dir_all(&dir).ok();
    }
}
