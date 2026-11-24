use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn key(path: &Path) -> IndexKey {
    IndexKey {
        root: std::fs::canonicalize(path).unwrap(),
        show_hidden: false,
        max_depth: None,
    }
}
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "").unwrap();
    std::fs::write(dir.path().join("node_modules/pkg/a.js"), "").unwrap();
    dir
}
fn slots(state: &FileIndexState) -> usize {
    state.0.table.lock().unwrap().slots.len()
}

#[test]
fn packed_round_trips_unicode_and_entry_kinds() {
    let mut packed = Packed::default();
    for (path, kind) in [
        ("src", EntryKind::Directory),
        ("src/中文.rs", EntryKind::File),
        ("link", EntryKind::Other),
    ] {
        assert!(packed.push(path, kind));
    }
    assert_eq!(packed.get(1), ("src/中文.rs", EntryKind::File));
    assert_eq!(packed.get(2), ("link", EntryKind::Other));
    assert_eq!(packed.iter().count(), 3);
}

#[test]
fn packed_bounds_data_and_allocation_capacity() {
    let mut packed = Packed::default();
    for _ in 0..MAX_SCANNED {
        assert!(packed.push("a", EntryKind::File));
    }
    assert!(!packed.push("extra", EntryKind::File));
    assert!(packed.ends.capacity() <= MAX_SCANNED);
    let mut packed = Packed::default();
    assert!(!packed.push(&"x".repeat(MAX_PACKED_BYTES + 1), EntryKind::File));
    let large = "x".repeat(MAX_PACKED_BYTES / 3);
    for _ in 0..3 {
        assert!(packed.push(&large, EntryKind::File));
    }
    assert!(packed.buf.capacity() <= MAX_PACKED_BYTES);
}

#[test]
fn walk_prunes_heavy_directories_and_respects_visibility_and_depth() {
    let dir = tree();
    std::fs::write(dir.path().join(".hidden"), "").unwrap();
    let mut k = key(dir.path());
    let index = build_index(&k);
    assert!(index.entries.iter().any(|(path, _)| path == "src/a.rs"));
    assert!(index
        .entries
        .iter()
        .all(|(path, _)| !path.contains("node_modules") && path != ".hidden"));
    k.show_hidden = true;
    k.max_depth = Some(1);
    let index = build_index(&k);
    assert!(index.entries.iter().any(|(path, _)| path == ".hidden"));
    assert!(index.entries.iter().all(|(path, _)| path != "src/a.rs"));
}

#[test]
fn fresh_queries_reuse_a_single_build_and_expire_on_ttl() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    let now = Instant::now();
    let builds = AtomicUsize::new(0);
    let get = |at| {
        state.get_or_build_with(&k, at, || {
            builds.fetch_add(1, Ordering::SeqCst);
            build_index(&k)
        })
    };
    let first = get(now);
    assert!(Arc::ptr_eq(&first, &get(now + Duration::from_secs(1))));
    get(now + TTL);
    assert_eq!(builds.load(Ordering::SeqCst), 2);
}

#[test]
fn concurrent_queries_share_the_cold_build() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    let barrier = std::sync::Barrier::new(4);
    let builds = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    state.get_or_build_with(&k, Instant::now(), || {
                        builds.fetch_add(1, Ordering::SeqCst);
                        build_index(&k)
                    })
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[test]
fn only_two_lru_entries_are_retained() {
    let dirs: Vec<_> = (0..3).map(|_| tree()).collect();
    let state = FileIndexState::default();
    let now = Instant::now();
    for (n, dir) in dirs.iter().enumerate() {
        let k = key(dir.path());
        state.get_or_build_with(&k, now + Duration::from_secs(n as u64), || build_index(&k));
    }
    assert_eq!(slots(&state), 2);
    assert!(state
        .0
        .table
        .lock()
        .unwrap()
        .slots
        .iter()
        .all(|slot| slot.key.root != key(dirs[0].path()).root));
}

#[test]
fn ignored_subtrees_do_not_invalidate_but_ignore_configuration_does() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    state.get_or_build(&k);
    state.note_change(&to_canon(k.root.join("node_modules/pkg/a.js")));
    state.note_change(&to_canon(k.root.join("target/debug")));
    state.note_change(&format!("{}-other/file", to_canon(&k.root)));
    assert_eq!(slots(&state), 1);
    state.note_change(&to_canon(k.root.join(".git/info/exclude")));
    assert_eq!(slots(&state), 0);
    state.get_or_build(&k);
    state.note_change(&to_canon(k.root.parent().unwrap()));
    assert_eq!(slots(&state), 0);
}

#[test]
fn a_change_during_a_walk_never_publishes_stale_data() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    let first = state.get_or_build_with(&k, Instant::now(), || {
        let index = build_index(&k);
        std::fs::write(k.root.join("late.rs"), "").unwrap();
        state.note_change(&to_canon(k.root.join("late.rs")));
        index
    });
    assert!(first.entries.iter().all(|(path, _)| path != "late.rs"));
    assert_eq!(slots(&state), 0);
    assert!(state
        .get_or_build(&k)
        .entries
        .iter()
        .any(|(path, _)| path == "late.rs"));
}

#[test]
fn ancestor_ignore_edits_invalidate_nested_roots_but_not_siblings() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(&dir.path().join("src"));
    for relative in [".gitignore", ".ignore", ".git/info/exclude", ".git/config"] {
        state.get_or_build(&k);
        state.note_change(&to_canon(dir.path().join("other").join(relative)));
        assert_eq!(slots(&state), 1);
        state.note_change(&to_canon(dir.path().join(relative)));
        assert_eq!(slots(&state), 0, "{relative}");
    }
}

#[test]
fn ancestor_ignore_edit_during_a_walk_prevents_publication() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(&dir.path().join("src"));
    state.get_or_build_with(&k, Instant::now(), || {
        let index = build_index(&k);
        state.note_change(&to_canon(dir.path().join(".gitignore")));
        index
    });
    assert_eq!(slots(&state), 0);
}

#[test]
fn panic_unwinds_walk_registration_and_the_next_build_still_works() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.get_or_build_with(
            &k,
            Instant::now(),
            || panic!("walk failed")
        )))
        .is_err()
    );
    assert!(state.0.table.lock().unwrap().walks.is_empty());
    state.get_or_build(&k);
    assert_eq!(slots(&state), 1);
}

#[test]
fn mutation_guard_invalidates_before_and_after_even_when_work_panics() {
    let dir = tree();
    let state = FileIndexState::default();
    let k = key(dir.path());
    state.get_or_build(&k);
    let path = to_canon(k.root.join("src/a.rs"));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _change = state.track_entries(&[&path], None);
        assert_eq!(slots(&state), 0);
        state.get_or_build(&k);
        panic!("mutation failed after changing disk");
    }))
    .is_err());
    assert_eq!(slots(&state), 0);
}

#[cfg(unix)]
#[test]
fn mutation_through_a_directory_alias_invalidates_the_canonical_root() {
    let dir = tree();
    let alias_parent = tempfile::tempdir().unwrap();
    let alias = alias_parent.path().join("alias");
    std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
    let state = FileIndexState::default();
    let k = key(dir.path());
    state.get_or_build(&k);
    let change = state.track_directory(&to_canon(&alias), None);
    assert_eq!(slots(&state), 0);
    state.get_or_build(&k);
    drop(change);
    assert_eq!(slots(&state), 0);
}
