//! Text a room client hands over with a message — a pasted log, a note written
//! on a phone — kept as a file in this machine's handoff folder, so it travels
//! the way every other reference does: agents on any machine fetch it, and the
//! assistant reads it.
//!
//! This is the one thing a room client may write, so it is fenced: text only,
//! into one subfolder, under a sanitised name that never replaces an existing
//! file, and within a size budget — a client token must not be a way to fill
//! this machine's disk.

use std::path::Path;

use serde::Deserialize;

use super::Ref;

/// Attachments per message.
const MAX_ATTACHMENTS: usize = 4;

/// One attachment's size: what [`crate::session::read_handoff_file`] serves back.
const MAX_ATTACHMENT: usize = 64 * 1024;

/// Everything attached, in total, before the folder refuses more until someone
/// clears it. Refusing loses nothing; quietly deleting old files would.
const FOLDER_BUDGET: u64 = 50 * 1024 * 1024;

/// Under `.phantom/handoff/`, where attachments land.
const FOLDER: &str = "room";

#[derive(Debug, Deserialize)]
pub struct Attachment {
    /// A name to keep it under. Sanitised; empty is fine.
    #[serde(default)]
    pub name: String,
    pub text: String,
}

/// Writes each attachment into `<workspace>/.phantom/handoff/room/` and returns
/// the references that name them, owned by `peer` — this machine.
pub fn store(
    workspace: Option<&Path>,
    peer: &str,
    attachments: Vec<Attachment>,
) -> Result<Vec<Ref>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(format!("at most {MAX_ATTACHMENTS} attachments per message"));
    }
    let root = workspace.ok_or("this server keeps no files; set room.workspace to accept attachments")?;
    let dir = root.join(".phantom/handoff").join(FOLDER);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    // All checked before any is written, so a refused message leaves nothing.
    let mut used = folder_size(&dir);
    for a in &attachments {
        if a.text.len() > MAX_ATTACHMENT {
            return Err(format!("{:?} is over {} KiB", a.name, MAX_ATTACHMENT / 1024));
        }
        // A NUL is the tell of a binary file read as text.
        if a.text.contains('\0') {
            return Err(format!("{:?} is not text", a.name));
        }
        used += a.text.len() as u64;
    }
    if used > FOLDER_BUDGET {
        return Err(format!(
            "the room's attachment folder is full ({} MiB); clear .phantom/handoff/{FOLDER}/",
            FOLDER_BUDGET / (1024 * 1024)
        ));
    }

    let mut refs = Vec::with_capacity(attachments.len());
    for a in attachments {
        let leaf = unused_name(&dir, &sanitize(&a.name));
        let path = dir.join(&leaf);
        std::fs::write(&path, a.text).map_err(|e| format!("write {}: {e}", path.display()))?;
        refs.push(Ref::Handoff { peer: peer.to_string(), file: format!("{FOLDER}/{leaf}") });
    }
    Ok(refs)
}

/// A name that is one plain path component: letters and digits in any script,
/// and `.`, `-`, `_`. Anything else becomes `-`, so `../../x` is just `x`.
fn sanitize(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' })
        .collect();
    let trimmed: String = mapped
        .trim_start_matches(['.', '-'])
        .chars()
        .take(80)
        .collect();
    if trimmed.is_empty() { "attachment.txt".to_string() } else { trimmed }
}

/// `name`, prefixed with the time, and numbered if even that is taken — an
/// attachment never replaces a file someone may still be reading.
fn unused_name(dir: &Path, name: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut candidate = format!("{secs}-{name}");
    let mut n = 2;
    while dir.join(&candidate).exists() {
        candidate = format!("{secs}-{n}-{name}");
        n += 1;
    }
    candidate
}

fn folder_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, text: &str) -> Attachment {
        Attachment { name: name.into(), text: text.into() }
    }

    #[test]
    fn a_name_can_never_leave_the_folder() {
        assert_eq!(sanitize("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize("构建日志.md"), "构建日志.md");
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize(""), "attachment.txt");
        assert_eq!(sanitize("a b/c"), "a-b-c");
    }

    #[test]
    fn attachments_become_references_this_machine_can_serve() {
        let root = tempfile::tempdir().unwrap();
        let refs = store(Some(root.path()), "alpha", vec![att("log.txt", "boom"), att("log.txt", "again")])
            .unwrap();
        assert_eq!(refs.len(), 2);
        let files: Vec<String> = refs
            .iter()
            .map(|r| match r {
                Ref::Handoff { peer, file } => {
                    assert_eq!(peer, "alpha");
                    file.clone()
                }
                Ref::Blob { .. } => unreachable!(),
            })
            .collect();
        // Same name twice: both kept.
        assert_ne!(files[0], files[1]);
        // And readable through the same guard every remote fetch uses.
        assert_eq!(crate::session::read_handoff_file(root.path(), &files[1]).unwrap(), "again");
    }

    #[test]
    fn what_a_client_may_not_write_is_refused_whole() {
        let root = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_ATTACHMENT + 1);
        assert!(store(Some(root.path()), "a", vec![att("ok.txt", "fine"), att("big.txt", &big)]).is_err());
        assert!(store(Some(root.path()), "a", vec![att("bin", "a\0b")]).is_err());
        let many = (0..=MAX_ATTACHMENTS).map(|i| att(&format!("{i}"), "x")).collect();
        assert!(store(Some(root.path()), "a", many).is_err());
        assert!(store(None, "a", vec![att("x", "y")]).is_err());
        // Nothing half-written by the refusals above.
        assert_eq!(folder_size(&root.path().join(".phantom/handoff/room")), 0);
    }
}
