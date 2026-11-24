//! The room's append-only log.
//!
//! One JSONL file, one message per line, plus an in-memory ring of the most
//! recent messages so `room_history` answers without touching the disk. JSONL
//! rather than sqlite because the access pattern is "append, and read the tail"
//! and nothing else — and a plain text log can be `tail -f`'d while debugging a
//! cross-machine handoff, which is exactly when you want to see it.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::RoomMsg;

/// How many recent messages stay in memory for `room_history`. A reconnecting
/// client asks for everything after the last `seq` it saw; anything older than
/// this window it simply doesn't get, which is the right trade for a chat log.
const RING_CAP: usize = 500;

/// Cap on the tail we read back at startup. A room log grows without bound, and
/// reading all of it to recover the last 500 lines would be silly.
const TAIL_BYTES: u64 = 1 << 22; // 4 MiB

pub struct RoomLog {
    path: PathBuf,
    ring: VecDeque<RoomMsg>,
    next_seq: u64,
    /// Which log this is. Sequence numbers start again whenever a log does —
    /// a renamed room, a deleted file, a hub moved to another box — so anyone
    /// holding messages has to be able to tell they came from a different one.
    epoch: String,
    /// Whether `epoch` is on disk yet. Written with the first message, not at
    /// open: a log with nothing in it needs no identity to keep.
    epoch_saved: bool,
}

impl RoomLog {
    /// Opens (creating the parent directory) and primes the ring from the tail.
    ///
    /// A corrupt or unreadable log is not fatal: the room starts empty rather
    /// than the server refusing to boot. Losing chat history is recoverable;
    /// losing the terminal that history is about is not.
    pub fn open(path: PathBuf) -> Self {
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::error!("room log dir {}: {e}", dir.display());
            }
        }
        let ring = read_tail(&path);
        let next_seq = ring.back().map_or(0, |m| m.seq + 1);
        // A log with nothing in it is a new log, whatever the epoch file left
        // beside it says: deleting the history must not keep its identity.
        let (epoch, epoch_saved) = match read_epoch(&path) {
            Some(epoch) if !ring.is_empty() => (epoch, true),
            _ => (new_epoch(), false),
        };
        let mut log = Self { path, ring, next_seq, epoch, epoch_saved };
        // History from before epochs existed: name it now, not on the next
        // message, or it would be a different log after every restart.
        if !log.ring.is_empty() {
            log.save_epoch();
        }
        log
    }

    fn save_epoch(&mut self) {
        if !self.epoch_saved {
            write_epoch(&self.path, &self.epoch);
            self.epoch_saved = true;
        }
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Peer side: starts mirroring a different log. What this machine held is
    /// set aside under its old epoch's name rather than deleted — the hub's
    /// copy may be the thing that was lost. True if there was anything to set
    /// aside, as opposed to a first join adopting the hub's log.
    pub fn restart_as(&mut self, epoch: &str) -> bool {
        let had_copy = !self.ring.is_empty();
        if had_copy {
            let aside = self.path.with_extension(format!("{}.jsonl", self.epoch));
            if let Err(e) = std::fs::rename(&self.path, &aside) {
                log::error!("setting {} aside: {e}", self.path.display());
            }
        }
        self.ring.clear();
        self.next_seq = 0;
        self.epoch = epoch.to_string();
        self.epoch_saved = false;
        had_copy
    }

    /// Claims the next sequence number. Monotonic within a process run and,
    /// because it resumes from the log's last line, across restarts too.
    pub fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Appends to the ring and to disk. A write failure is logged, not
    /// propagated: the message is already live in the ring and on its way to
    /// every subscriber, so failing the post would be a lie.
    pub fn append(&mut self, msg: RoomMsg) {
        if self.ring.len() == RING_CAP {
            self.ring.pop_front();
        }
        let line = match serde_json::to_string(&msg) {
            Ok(l) => l,
            Err(e) => {
                log::error!("serializing room message: {e}");
                self.ring.push_back(msg);
                return;
            }
        };
        self.ring.push_back(msg);
        self.save_epoch();
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = written {
            log::error!("appending to {}: {e}", self.path.display());
        }
    }

    /// The newest message's sequence, if any.
    pub fn last_seq(&self) -> Option<u64> {
        self.ring.back().map(|m| m.seq)
    }

    /// Everything after `seq`, oldest first. `None` means "the whole window",
    /// which is what a client asks for on its first load.
    pub fn since(&self, seq: Option<u64>) -> Vec<RoomMsg> {
        match seq {
            Some(s) => self.ring.iter().filter(|m| m.seq > s).cloned().collect(),
            None => self.ring.iter().cloned().collect(),
        }
    }
}

fn epoch_path(log: &Path) -> PathBuf {
    log.with_extension("epoch")
}

fn read_epoch(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(epoch_path(log)).ok()?;
    let epoch = text.trim();
    (!epoch.is_empty()).then(|| epoch.to_string())
}

fn write_epoch(log: &Path, epoch: &str) {
    if let Err(e) = std::fs::write(epoch_path(log), epoch) {
        log::error!("writing {}: {e}", epoch_path(log).display());
    }
}

fn new_epoch() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        // Unique enough to tell two logs apart, which is all it is for.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes = (nanos as u64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads up to the last [`RING_CAP`] well-formed messages.
///
/// Seeks to the last [`TAIL_BYTES`] and drops the first (probably partial)
/// line. Malformed lines are skipped rather than fatal — a half-written line
/// from a killed process must not cost the whole history.
fn read_tail(path: &Path) -> VecDeque<RoomMsg> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return VecDeque::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let partial_first = len > TAIL_BYTES;
    if partial_first && file.seek(SeekFrom::End(-(TAIL_BYTES as i64))).is_err() {
        return VecDeque::new();
    }
    let mut buf = String::new();
    // Lossy on purpose: one bad byte shouldn't discard the log.
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return VecDeque::new();
    }
    buf.push_str(&String::from_utf8_lossy(&bytes));

    let mut lines = buf.lines();
    if partial_first {
        lines.next();
    }
    let mut ring = VecDeque::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RoomMsg>(line) {
            Ok(msg) => {
                if ring.len() == RING_CAP {
                    ring.pop_front();
                }
                ring.push_back(msg);
            }
            Err(e) => log::warn!("skipping malformed room log line: {e}"),
        }
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::Participant;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "phantom-room-{tag}-{}-{:?}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn msg(seq: u64, text: &str) -> RoomMsg {
        RoomMsg {
            seq,
            ts: 0,
            from: Participant::human("local"),
            mentions: Vec::new(),
            text: text.into(),
            refs: Vec::new(),
            kind: Default::default(),
        }
    }

    #[test]
    fn seq_is_monotonic_and_resumes_across_reopen() {
        let path = tmp("seq");
        let mut log = RoomLog::open(path.clone());
        assert_eq!(log.next_seq(), 0);
        assert_eq!(log.next_seq(), 1);
        log.append(msg(1, "hi"));

        // A restart must not hand out a seq the log already used, or a
        // reconnecting client's `since` filter would silently drop messages.
        let mut reopened = RoomLog::open(path.clone());
        assert_eq!(reopened.next_seq(), 2);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("epoch")).ok();
    }

    #[test]
    fn since_filters_and_round_trips_through_disk() {
        let path = tmp("since");
        let mut log = RoomLog::open(path.clone());
        for i in 0..3 {
            log.append(msg(i, &format!("m{i}")));
        }
        assert_eq!(log.since(None).len(), 3);
        let tail = log.since(Some(0));
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].text, "m1");

        let reopened = RoomLog::open(path.clone());
        assert_eq!(reopened.since(None).len(), 3);
        assert_eq!(reopened.since(Some(1))[0].text, "m2");
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("epoch")).ok();
    }

    #[test]
    fn malformed_line_is_skipped_not_fatal() {
        let path = tmp("bad");
        let mut log = RoomLog::open(path.clone());
        log.append(msg(0, "good"));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{not json\n")
            .unwrap();
        let reopened = RoomLog::open(path.clone());
        assert_eq!(reopened.since(None).len(), 1);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("epoch")).ok();
    }

    #[test]
    fn a_log_keeps_its_epoch_until_it_is_a_new_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("room.jsonl");
        let mut log = RoomLog::open(path.clone());
        let first = log.epoch().to_string();
        log.append(msg(0, "hi"));
        // Reopened with its history: the same log.
        assert_eq!(RoomLog::open(path.clone()).epoch(), first);
        // History deleted, epoch file left behind: a new log all the same, or
        // everyone holding the old messages would mistake the new ones for them.
        std::fs::remove_file(&path).unwrap();
        assert_ne!(RoomLog::open(path.clone()).epoch(), first);
    }

    #[test]
    fn a_mirror_restarting_as_another_log_sets_the_old_one_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mirror.jsonl");
        let mut log = RoomLog::open(path.clone());
        let old = log.epoch().to_string();
        log.append(msg(41, "from the old hub log"));

        log.restart_as("feedface");
        assert_eq!(log.epoch(), "feedface");
        assert!(log.since(None).is_empty());
        assert!(dir.path().join(format!("mirror.{old}.jsonl")).exists(), "kept, not deleted");
        // And it stays that log across a restart of this machine.
        log.append(msg(0, "from the new one"));
        assert_eq!(RoomLog::open(path).epoch(), "feedface");
    }

    #[test]
    fn ring_is_capped() {
        let path = tmp("cap");
        let mut log = RoomLog::open(path.clone());
        for i in 0..(RING_CAP as u64 + 10) {
            log.append(msg(i, "x"));
        }
        assert_eq!(log.since(None).len(), RING_CAP);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("epoch")).ok();
    }
}
