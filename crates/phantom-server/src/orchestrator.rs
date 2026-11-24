//! The pluggable orchestrator brain (step 5 of the multi-CLI design).
//!
//! An external LLM (the user's GLM, a cloud API, a local Qwen — anything that
//! speaks the OpenAI chat-completions + tool-calling shape) drives the SAME
//! `dispatch`/`nudge` primitives the manual panel does. This module is only the
//! brain's I/O: config, the one-round-trip call, and the tool schema. The loop
//! that executes the tool calls lives on `Connection`, because it needs the
//! rmux sessions and the workspace.
//!
//! Content between agents still travels by file (`.phantom/handoff/*.md`); the
//! brain reads those and pokes sessions, it does not shuttle payloads itself.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

/// Where the brain lives, from the `orchestrator:` block of `.phantom/agents.yaml`.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub endpoint: String,
    pub model: String,
}

#[derive(Deserialize)]
struct RawOrchestrator {
    /// Defaults to GLM's OpenAI-compatible endpoint when omitted.
    #[serde(default)]
    endpoint: Option<String>,
    model: String,
}

#[derive(Deserialize)]
struct RawConfigWithOrch {
    #[serde(default)]
    orchestrator: Option<RawOrchestrator>,
}

/// GLM's OpenAI-compatible chat endpoint — the common default so a config only
/// has to name a model.
pub const DEFAULT_ENDPOINT: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";

/// Reads the `orchestrator:` block. `None` (no block) means the workspace hasn't
/// wired a brain — the panel then offers only manual control.
pub fn load_config(root: &Path) -> Result<Option<OrchestratorConfig>, String> {
    let path = root.join(".phantom/agents.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let cfg: RawConfigWithOrch =
        serde_yaml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(cfg.orchestrator.map(|o| OrchestratorConfig {
        endpoint: o.endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
        model: o.model,
    }))
}

/// One assistant turn: either it asked to call tools, or it wrote prose.
pub struct Assistant {
    /// Verbatim assistant message, to append to the running transcript.
    pub message: Value,
    /// Parsed tool calls (id, name, arguments), empty if it just wrote text.
    pub tool_calls: Vec<ToolCall>,
    /// Assistant prose when there were no tool calls.
    pub content: String,
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// The tools the brain may call. Names match what `Connection` executes.
pub fn tools() -> Value {
    let f = |name: &str, desc: &str, props: Value, required: Vec<&str>| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": desc,
                "parameters": {
                    "type": "object",
                    "properties": props,
                    "required": required,
                },
            },
        })
    };
    json!([
        f("dispatch_agent",
          "Send a task to an agent's terminal. Tell it concretely what to do and which .phantom/handoff/<file>.md to write its result or error to.",
          json!({"agent": {"type": "string"}, "task": {"type": "string"}}),
          vec!["agent", "task"]),
        f("nudge_agent",
          "Like dispatch_agent, but waits until the agent is idle before sending, so it lands at a prompt rather than interrupting a running command.",
          json!({"agent": {"type": "string"}, "message": {"type": "string"}}),
          vec!["agent", "message"]),
        f("read_handoff",
          "Read a file under .phantom/handoff/ to see what an agent produced.",
          json!({"file": {"type": "string"}}),
          vec!["file"]),
        f("list_agents",
          "List the agents, their roles, and whether each has a live session.",
          json!({}),
          vec![]),
        f("finish",
          "Call when the goal is met (or cannot proceed) with a short summary.",
          json!({"summary": {"type": "string"}}),
          vec!["summary"]),
    ])
}

/// The standing instructions that make the brain use files + primitives right.
pub fn system_prompt(roster: &str) -> String {
    format!(
        "You orchestrate a team of CLI coding agents. Each runs in its own \
terminal; you cannot see their screens. They collaborate by writing files under \
.phantom/handoff/.\n\n\
Workflow:\n\
- dispatch_agent to instruct an agent. ALWAYS tell it exactly which \
.phantom/handoff/<file>.md to write when it finishes, and to write DONE there.\n\
- read_handoff to check a result. If the file is missing or empty, the agent is \
STILL WORKING — coding tasks take minutes and you cannot wait them out. Do NOT \
read the same file again in a loop. Instead call finish, noting what you \
dispatched and that the human should re-run you once the handoff file exists.\n\
- nudge_agent only to poke an already-idle agent to pick up a handoff.\n\
- Keep each instruction concrete and self-contained.\n\n\
Never repeat an identical read_handoff. Once the work is dispatched (or the goal \
is met), call finish — a fresh run will continue from the handoff files.\n\n\
Agents:\n{roster}"
    )
}

/// One round-trip to the brain. Non-streaming: tool-call parsing is simpler and
/// the panel streams our *steps*, not the model's tokens.
pub async fn call(
    client: &reqwest::Client,
    config: &OrchestratorConfig,
    key: &str,
    messages: &[Value],
    tools: &Value,
) -> Result<Assistant, String> {
    let body = json!({
        "model": config.model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
    });
    let message = request(client, config, key, &body).await?;

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for c in calls {
            let id = c.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
            let name = c
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            // Arguments arrive as a JSON *string*; parse it, tolerating empties.
            let raw = c
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let arguments: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
            tool_calls.push(ToolCall { id, name, arguments });
        }
    }
    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    Ok(Assistant { message, tool_calls, content })
}

/// One plain chat turn, no tools: the model's answer as text.
pub async fn chat(
    client: &reqwest::Client,
    config: &OrchestratorConfig,
    key: &str,
    messages: &[Value],
) -> Result<String, String> {
    let body = json!({ "model": config.model, "messages": messages });
    let message = request(client, config, key, &body).await?;
    Ok(message.get("content").and_then(Value::as_str).unwrap_or("").to_string())
}

/// Posts one chat-completions request and returns the first choice's message.
async fn request(
    client: &reqwest::Client,
    config: &OrchestratorConfig,
    key: &str,
    body: &Value,
) -> Result<Value, String> {
    // Retry transient send failures (a flaky proxy, a reset). A single blip
    // shouldn't abort a whole multi-step orchestration. Only the *connection*
    // is retried — an HTTP error status below is a real API response, not
    // retried. Short linear backoff.
    let mut last_err = String::new();
    let mut resp = None;
    for attempt in 0..3 {
        match client
            .post(&config.endpoint)
            .bearer_auth(key)
            .json(body)
            .send()
            .await
        {
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(e) => {
                last_err = format!("orchestrator request failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(400 * (attempt + 1))).await;
            }
        }
    }
    let resp = resp.ok_or(last_err)?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("reading orchestrator response failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("orchestrator HTTP {status}: {text}"));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("orchestrator returned non-JSON ({e}): {text}"))?;
    v.pointer("/choices/0/message")
        .cloned()
        .ok_or_else(|| format!("orchestrator response had no message: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_orchestrator_block_is_none() {
        let dir = std::env::temp_dir().join(format!("phantom-orch-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".phantom")).unwrap();
        std::fs::write(dir.join(".phantom/agents.yaml"), "agents: {}\n").unwrap();
        assert!(load_config(&dir).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_orchestrator_and_defaults_endpoint() {
        let dir = std::env::temp_dir().join(format!("phantom-orch2-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".phantom")).unwrap();
        std::fs::write(
            dir.join(".phantom/agents.yaml"),
            "orchestrator:\n  model: glm-4.6\n",
        )
        .unwrap();
        let cfg = load_config(&dir).unwrap().unwrap();
        assert_eq!(cfg.model, "glm-4.6");
        assert_eq!(cfg.endpoint, DEFAULT_ENDPOINT);
        std::fs::remove_dir_all(&dir).ok();
    }
}
