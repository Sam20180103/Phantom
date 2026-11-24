use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;

use super::to_canon;
use crate::workspace::{resolve_path, WorkspaceEnv};

pub const MAX_SCANNED: usize = 50_000;
pub const PRUNE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    ".venv",
    "__pycache__",
];
const MAX_SLOTS: usize = 2;
const TTL: Duration = Duration::from_secs(45);
const MAX_PACKED_BYTES: usize = 16 * 1024 * 1024;
const DIR_BIT: u32 = 1 << 31;
const FILE_BIT: u32 = 1 << 30;
const OFFSET_MASK: u32 = !(DIR_BIT | FILE_BIT);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Other,
}

#[derive(Default)]
pub struct Packed {
    buf: String,
    ends: Vec<u32>,
}

impl Packed {
    pub(super) fn push(&mut self, rel: &str, kind: EntryKind) -> bool {
        if rel.len() > MAX_PACKED_BYTES.saturating_sub(self.buf.len())
            || self.ends.len() >= MAX_SCANNED
        {
            return false;
        }
        let needed = self.buf.len() + rel.len();
        if needed > self.buf.capacity() {
            let capacity = needed
                .max(self.buf.capacity().saturating_mul(2))
                .min(MAX_PACKED_BYTES);
            self.buf.reserve_exact(capacity - self.buf.len());
        }
        if self.ends.len() == self.ends.capacity() {
            let capacity = self
                .ends
                .capacity()
                .saturating_mul(2)
                .clamp(16, MAX_SCANNED);
            self.ends.reserve_exact(capacity - self.ends.len());
        }
        self.buf.push_str(rel);
        let flag = match kind {
            EntryKind::File => FILE_BIT,
            EntryKind::Directory => DIR_BIT,
            EntryKind::Other => 0,
        };
        self.ends.push(self.buf.len() as u32 | flag);
        true
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
    pub fn get(&self, index: usize) -> (&str, EntryKind) {
        let start = if index == 0 {
            0
        } else {
            (self.ends[index - 1] & OFFSET_MASK) as usize
        };
        let raw = self.ends[index];
        let kind = if raw & DIR_BIT != 0 {
            EntryKind::Directory
        } else if raw & FILE_BIT != 0 {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        (&self.buf[start..(raw & OFFSET_MASK) as usize], kind)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, EntryKind)> {
        (0..self.len()).map(|i| self.get(i))
    }
}

pub struct FileIndex {
    pub entries: Packed,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexKey {
    pub root: PathBuf,
    pub show_hidden: bool,
    pub max_depth: Option<usize>,
}

struct Slot {
    key: IndexKey,
    index: Arc<FileIndex>,
    built_at: Instant,
    last_used: Instant,
}
struct Walk {
    id: u64,
    root: String,
    dirty: bool,
}
#[derive(Default)]
struct Table {
    slots: Vec<Slot>,
    walks: Vec<Walk>,
    next_walk: u64,
}
#[derive(Default)]
struct Inner {
    table: Mutex<Table>,
    build: Mutex<()>,
}
#[derive(Clone, Default)]
pub struct FileIndexState(Arc<Inner>);

struct WalkGuard<'a> {
    inner: &'a Inner,
    id: u64,
}
impl Drop for WalkGuard<'_> {
    fn drop(&mut self) {
        self.inner
            .table
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .walks
            .retain(|walk| walk.id != self.id);
    }
}

fn covers(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    if path == root {
        return true;
    }
    for suffix in [
        "/.gitignore",
        "/.ignore",
        "/.git/info/exclude",
        "/.git/config",
    ] {
        if let Some(scope) = path.strip_suffix(suffix) {
            if root == scope
                || root
                    .strip_prefix(scope)
                    .is_some_and(|rest| rest.starts_with('/'))
            {
                return true;
            }
        }
    }
    let prefix = format!("{root}/");
    if let Some(relative) = path.strip_prefix(&prefix) {
        if relative == ".git/info/exclude" || relative == ".git/config" {
            return true;
        }
        return !relative.split('/').any(|part| PRUNE_DIRS.contains(&part));
    }
    root.strip_prefix(path)
        .is_some_and(|rest| rest.starts_with('/'))
}

impl FileIndexState {
    pub fn get_or_build(&self, key: &IndexKey) -> Arc<FileIndex> {
        self.get_or_build_with(key, Instant::now(), || build_index(key))
    }

    fn get_or_build_with(
        &self,
        key: &IndexKey,
        now: Instant,
        build: impl FnOnce() -> FileIndex,
    ) -> Arc<FileIndex> {
        if let Some(index) = self.lookup(key, now) {
            return index;
        }
        let _build = self.0.build.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = self.lookup(key, now) {
            return index;
        }
        let id = {
            let mut table = self.0.table.lock().unwrap_or_else(|e| e.into_inner());
            let id = table.next_walk;
            table.next_walk = table.next_walk.wrapping_add(1);
            table.walks.push(Walk {
                id,
                root: to_canon(&key.root),
                dirty: false,
            });
            id
        };
        let _walk = WalkGuard { inner: &self.0, id };
        let index = Arc::new(build());
        let mut table = self.0.table.lock().unwrap_or_else(|e| e.into_inner());
        // Checking dirtiness and publishing share a lock with invalidation.
        if table.walks.iter().any(|walk| walk.id == id && !walk.dirty) {
            table.slots.retain(|slot| {
                slot.key != *key && now.saturating_duration_since(slot.built_at) < TTL
            });
            if table.slots.len() >= MAX_SLOTS {
                let oldest = table
                    .slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.last_used)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                table.slots.swap_remove(oldest);
            }
            table.slots.push(Slot {
                key: key.clone(),
                index: index.clone(),
                built_at: now,
                last_used: now,
            });
        }
        index
    }

    fn lookup(&self, key: &IndexKey, now: Instant) -> Option<Arc<FileIndex>> {
        let mut table = self.0.table.lock().unwrap_or_else(|e| e.into_inner());
        table
            .slots
            .retain(|slot| now.saturating_duration_since(slot.built_at) < TTL);
        let slot = table.slots.iter_mut().find(|slot| slot.key == *key)?;
        slot.last_used = now;
        Some(slot.index.clone())
    }

    pub fn note_change(&self, path: &str) {
        let mut table = self.0.table.lock().unwrap_or_else(|e| e.into_inner());
        if table.slots.is_empty() && table.walks.is_empty() {
            return;
        }
        let path = path.replace('\\', "/");
        for walk in &mut table.walks {
            if covers(&walk.root, &path) {
                walk.dirty = true;
            }
        }
        table
            .slots
            .retain(|slot| !covers(&to_canon(&slot.key.root), &path));
    }

    pub fn track_entries(&self, paths: &[&str], workspace: Option<&WorkspaceEnv>) -> ChangeGuard {
        self.track(paths, workspace, false)
    }
    pub fn track_directory(&self, path: &str, workspace: Option<&WorkspaceEnv>) -> ChangeGuard {
        self.track(&[path], workspace, true)
    }
    fn track(&self, paths: &[&str], workspace: Option<&WorkspaceEnv>, follow: bool) -> ChangeGuard {
        let workspace = workspace.cloned().unwrap_or_default();
        let paths: Vec<_> = paths
            .iter()
            .map(|path| resolve_path(path, &workspace))
            .collect();
        let before: Vec<_> = paths.iter().map(|path| event_path(path, follow)).collect();
        for path in &before {
            self.note_change(path);
        }
        ChangeGuard {
            state: self.clone(),
            paths,
            before,
            follow,
        }
    }
}

#[must_use]
pub struct ChangeGuard {
    state: FileIndexState,
    paths: Vec<PathBuf>,
    before: Vec<String>,
    follow: bool,
}
impl Drop for ChangeGuard {
    fn drop(&mut self) {
        for path in &self.before {
            self.state.note_change(path);
        }
        for path in &self.paths {
            self.state.note_change(&event_path(path, self.follow));
        }
    }
}

fn event_path(path: &Path, follow: bool) -> String {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    if follow {
        if let Ok(path) = std::fs::canonicalize(&absolute) {
            return to_canon(path);
        }
    }
    for ancestor in absolute.parent().unwrap_or(&absolute).ancestors() {
        if let Ok(parent) = std::fs::canonicalize(ancestor) {
            if let Ok(tail) = absolute.strip_prefix(ancestor) {
                return to_canon(parent.join(tail));
            }
        }
    }
    to_canon(absolute)
}

fn build_index(key: &IndexKey) -> FileIndex {
    let walker = WalkBuilder::new(&key.root)
        .hidden(!key.show_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .max_depth(key.max_depth)
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !PRUNE_DIRS.contains(&name))
        })
        .build();
    let mut entries = Packed::default();
    let mut truncated = false;
    for (scanned, entry) in walker.flatten().enumerate() {
        if scanned >= MAX_SCANNED {
            truncated = true;
            break;
        }
        let Ok(relative) = entry.path().strip_prefix(&key.root) else {
            continue;
        };
        let relative = to_canon(relative);
        if relative.is_empty() {
            continue;
        }
        let kind = match entry.file_type() {
            Some(kind) if kind.is_dir() => EntryKind::Directory,
            Some(kind) if kind.is_file() => EntryKind::File,
            _ => EntryKind::Other,
        };
        if !entries.push(&relative, kind) {
            truncated = true;
            break;
        }
    }
    FileIndex { entries, truncated }
}

#[cfg(test)]
mod tests;
