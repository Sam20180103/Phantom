//! The room's built-in assistant: a mention answered by a model, not a CLI.
//!
//! Everything else in the room reaches agents that run somewhere — installed,
//! signed in, started. A phone or a borrowed laptop has none of that, and the
//! room was only a way to reach someone else's. This answers directly: the
//! question, the recent room around it and any handoff files attached to it
//! go to an OpenAI-compatible model through the orchestrator's client, and the
//! answer is posted back into the room under the assistant's name.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{peer, MsgKind, Participant, Ref, RoomMsg, MAX_TEXT};
use crate::config::Assistant;
use crate::AppState;

/// Earlier room messages the model sees, so "and the other one?" means something.
const CONTEXT_MESSAGES: usize = 20;

/// Attached file text passed to the model, in total across all attachments.
const MAX_ATTACHED: usize = 48 * 1024;

/// Answers `question`, which mentioned the assistant. Posts the answer, or a
/// notice saying why there is none.
pub async fn answer(state: Arc<AppState>, question: RoomMsg) {
    let Some(assistant) = state.config.room.assistant.as_ref() else {
        return;
    };
    let me = Participant::agent(&state.rooms.local_peer, &assistant.name);
    // Shown as working in the roster while it thinks: a person who asked
    // should see it was heard.
    let _busy = state.rooms.assistant_working();
    let reply = match ask(&state, assistant, &question).await {
        Ok(reply) if !reply.trim().is_empty() => fit(reply),
        Ok(_) => {
            let why = "the model returned an empty answer";
            return state.rooms.notice(format!("{} could not answer: {why}", me.display()));
        }
        Err(e) => {
            log::warn!("{} could not answer: {e}", me.display());
            // The API's own error text can run long; the first line says enough.
            let why: String = e.lines().next().unwrap_or_default().chars().take(300).collect();
            return state.rooms.notice(format!("{} could not answer: {why}", me.display()));
        }
    };
    match state.rooms.post(me, reply, Vec::new()) {
        Ok(Some(msg)) => peer::fanout(&state, &msg),
        Ok(None) => {} // a peer's answer, sequenced by the hub
        Err(e) => log::warn!("posting the assistant's answer failed: {e}"),
    }
}

async fn ask(state: &Arc<AppState>, assistant: &Assistant, question: &RoomMsg) -> Result<String, String> {
    let brain = brain(state, assistant)?;
    let key = state
        .secrets
        .get("orchestrator", "apiKey")?
        .or_else(|| std::env::var("PHANTOM_ORCHESTRATOR_KEY").ok())
        .ok_or("no API key: set the orchestrator/apiKey secret or PHANTOM_ORCHESTRATOR_KEY")?;
    let attached = attachments(state, &question.refs).await;
    let messages = prompt(
        &state.rooms.title(),
        &assistant.name,
        &state.rooms.history(None),
        question,
        &attached,
    );
    crate::orchestrator::chat(&reqwest::Client::new(), &brain, &key, &messages).await
}

/// Which model to ask: the assistant's own, else the workspace's orchestrator.
fn brain(
    state: &AppState,
    assistant: &Assistant,
) -> Result<crate::orchestrator::OrchestratorConfig, String> {
    if let Some(model) = &assistant.model {
        return Ok(crate::orchestrator::OrchestratorConfig {
            endpoint: assistant
                .endpoint
                .clone()
                .unwrap_or_else(|| crate::orchestrator::DEFAULT_ENDPOINT.to_string()),
            model: model.clone(),
        });
    }
    let workspace = state.config.room.workspace.as_deref();
    workspace
        .map(crate::orchestrator::load_config)
        .transpose()?
        .flatten()
        .ok_or_else(|| "no model: set room.assistant.model".to_string())
}

/// The text of each attached handoff file, fetched from whichever machine owns
/// it, as `(name, text)`. A file that can't be read is still named, so the
/// model doesn't answer as though nothing was attached.
async fn attachments(state: &Arc<AppState>, refs: &[Ref]) -> Vec<(String, String)> {
    let mut left = MAX_ATTACHED;
    let mut out = Vec::new();
    for r in refs {
        // Blobs are bytes; there is nothing here to read them as.
        let Ref::Handoff { peer, file } = r else { continue };
        let name = format!("{peer}:{file}");
        if left == 0 {
            out.push((name, "(not included: attachments too long)".to_string()));
            continue;
        }
        let text = match peer::resolve_ref(state, r.clone()).await {
            Ok(mut text) => {
                if text.len() > left {
                    text.truncate(text.floor_char_boundary(left));
                    text.push_str("\n…(truncated)");
                }
                left = left.saturating_sub(text.len());
                text
            }
            Err(e) => format!("(could not be read: {e})"),
        };
        out.push((name, text));
    }
    out
}

/// The conversation sent to the model: standing instructions, then one user
/// turn carrying the recent room, the attachments and the question.
fn prompt(
    room: &str,
    name: &str,
    history: &[RoomMsg],
    question: &RoomMsg,
    attached: &[(String, String)],
) -> Vec<Value> {
    let system = format!(
        "You are {name}, the built-in assistant of the room \"{room}\": a shared chat where \
         people and CLI coding agents on several machines hand each other work. Answer the \
         latest message addressed to you, directly and concisely, in the language it was \
         written in. Your answer is posted into the room as plain text. Put code, and \
         anything starting with @, inside backticks: a bare @name in your answer hands work \
         to that participant, so write one only when that is what you mean to do."
    );

    let earlier: Vec<&RoomMsg> = history
        .iter()
        .filter(|m| m.kind == MsgKind::Say && m.seq < question.seq)
        .collect();
    let mut user = String::new();
    if !earlier.is_empty() {
        user.push_str("Recent messages in the room, oldest first:\n");
        for m in &earlier[earlier.len().saturating_sub(CONTEXT_MESSAGES)..] {
            user.push_str(&format!("{}: {}\n", m.from.display(), m.text));
        }
        user.push('\n');
    }
    for (file, text) in attached {
        user.push_str(&format!("Attached file {file}:\n{}\n\n", text.trim_end()));
    }
    user.push_str(&format!("{} asks: {}", question.from.display(), question.text));

    vec![json!({"role": "system", "content": system}), json!({"role": "user", "content": user})]
}

/// Keeps an answer within what the room accepts, cut on a character boundary.
fn fit(mut reply: String) -> String {
    const MARK: &str = "\n…(truncated)";
    if reply.len() > MAX_TEXT {
        reply.truncate(reply.floor_char_boundary(MAX_TEXT - MARK.len()));
        reply.push_str(MARK);
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(seq: u64, from: Participant, text: &str, kind: MsgKind) -> RoomMsg {
        RoomMsg { seq, ts: 0, from, mentions: vec![], text: text.into(), refs: vec![], kind }
    }

    #[test]
    fn the_model_sees_the_room_before_the_question_and_nothing_after() {
        let history = vec![
            msg(1, Participant::human("b"), "the build broke", MsgKind::Say),
            msg(2, Participant::human("alpha"), "could not deliver to b/agy", MsgKind::Notice),
            msg(3, Participant::agent("b", "agy"), "fixed in api.md", MsgKind::Say),
            msg(4, Participant::human("b"), "@ai what changed?", MsgKind::Say),
            msg(5, Participant::human("b"), "never mind", MsgKind::Say),
        ];
        let attached = vec![("b:api.md".to_string(), "GET /v1/items".to_string())];
        let messages = prompt("main", "ai", &history, &history[3], &attached);
        let user = messages[1]["content"].as_str().unwrap();

        assert!(user.contains("b/human: the build broke\nb/agy: fixed in api.md\n"), "{user}");
        // Notices are the room talking about itself, not context.
        assert!(!user.contains("could not deliver"));
        // Only what came before the question.
        assert!(!user.contains("never mind"));
        assert!(user.contains("Attached file b:api.md:\nGET /v1/items"));
        assert!(user.ends_with("b/human asks: @ai what changed?"));
        // It must know a bare @name dispatches work.
        assert!(messages[0]["content"].as_str().unwrap().contains("backticks"));
    }

    #[test]
    fn a_long_answer_is_cut_to_fit_the_room() {
        let fitted = fit("答".repeat(MAX_TEXT)); // 3 bytes each: far over, and cut mid-character
        assert!(fitted.len() <= MAX_TEXT);
        assert!(fitted.ends_with("…(truncated)"));
        assert_eq!(fit("short".into()), "short");
    }
}
