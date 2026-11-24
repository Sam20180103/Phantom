use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;

use super::index::{EntryKind, FileIndexState, IndexKey, Packed};
use super::to_canon;
use crate::workspace::{resolve_path, WorkspaceEnv};

#[derive(Serialize, Clone)]
pub struct SearchHit {
    /// Absolute path of the matched file.
    pub path: String,
    /// Path relative to the search root, for display.
    pub rel: String,
    /// File name only.
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    /// True if the scan stopped early (entry budget or hit cap reached).
    pub truncated: bool,
}

pub fn fs_search(
    index: &FileIndexState,
    root: String,
    query: String,
    limit: Option<usize>,
    workspace: Option<WorkspaceEnv>,
    show_hidden: Option<bool>,
) -> Result<SearchResult, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(SearchResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let cap = limit.unwrap_or(200).min(1000);
    if cap == 0 {
        return Ok(SearchResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let show_hidden = show_hidden.unwrap_or(false);
    let workspace = WorkspaceEnv::from_option(workspace);
    let root_path = resolve_path(&root, &workspace);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }

    let file_index = index.get_or_build(&IndexKey {
        root: std::fs::canonicalize(&root_path).map_err(|error| error.to_string())?,
        show_hidden,
        max_depth: None,
    });
    let hits = rank_fuzzy(&file_index.entries, q, cap)
        .into_iter()
        .map(|i| {
            let (rel, kind) = file_index.entries.get(i);
            SearchHit {
                path: display_path(&root_path.join(rel), &root_path, &root, &workspace),
                rel: rel.to_string(),
                name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
                is_dir: kind == EntryKind::Directory,
            }
        })
        .collect();
    Ok(SearchResult {
        hits,
        truncated: file_index.truncated,
    })
}

/// Fuzzy-rank candidates against the query (path-aware, smart-case), keeping
/// the top `cap`. Ties break toward shorter relative paths.
fn rank_fuzzy(cands: &Packed, query: &str, cap: usize) -> Vec<usize> {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();

    let mut scored = Vec::with_capacity(cands.len());
    for (i, (rel, _)) in cands.iter().enumerate() {
        if let Some(s) = pattern.score(Utf32Str::new(rel, &mut buf), &mut matcher) {
            scored.push((s, i));
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| cands.get(a.1).0.len().cmp(&cands.get(b.1).0.len()))
    });
    scored.into_iter().take(cap).map(|(_, i)| i).collect()
}

#[derive(Serialize)]
pub struct ListFilesResult {
    pub files: Vec<String>,
    pub truncated: bool,
}

pub fn fs_list_files(
    index: &FileIndexState,
    root: String,
    limit: Option<usize>,
    max_depth: Option<usize>,
    workspace: Option<WorkspaceEnv>,
    show_hidden: Option<bool>,
) -> Result<ListFilesResult, String> {
    const DEFAULT_LIMIT: usize = 2_000;
    const HARD_LIMIT: usize = 10_000;
    const DEFAULT_DEPTH: usize = 8;
    const HARD_DEPTH: usize = 16;

    let cap = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, HARD_LIMIT);
    let depth = max_depth.unwrap_or(DEFAULT_DEPTH).clamp(1, HARD_DEPTH);
    let show_hidden = show_hidden.unwrap_or(false);
    let workspace = WorkspaceEnv::from_option(workspace);
    let root_path = resolve_path(&root, &workspace);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }

    let file_index = index.get_or_build(&IndexKey {
        root: std::fs::canonicalize(&root_path).map_err(|error| error.to_string())?,
        show_hidden,
        max_depth: Some(depth),
    });
    let mut candidates = file_index
        .entries
        .iter()
        .filter(|(_, kind)| *kind == EntryKind::File);
    let mut files: Vec<_> = candidates
        .by_ref()
        .take(cap)
        .map(|(rel, _)| rel.to_string())
        .collect();
    let truncated = file_index.truncated || candidates.next().is_some();
    files.sort_by_cached_key(|a| a.to_lowercase());
    Ok(ListFilesResult { files, truncated })
}

fn display_path(
    path: &std::path::Path,
    root_path: &std::path::Path,
    root_display: &str,
    workspace: &WorkspaceEnv,
) -> String {
    if workspace.is_wsl() {
        if let Ok(rel) = path.strip_prefix(root_path) {
            let rel = to_canon(rel);
            return if rel.is_empty() {
                root_display.to_string()
            } else if root_display.ends_with('/') {
                format!("{root_display}{rel}")
            } else {
                format!("{root_display}/{rel}")
            };
        }
    }
    to_canon(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_queries_see_new_names_only_after_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.rs"), "").unwrap();
        let root = to_canon(dir.path());
        let index = FileIndexState::default();
        let search = || fs_search(&index, root.clone(), "alpha".into(), None, None, None).unwrap();
        assert_eq!(search().hits.len(), 1);
        std::fs::write(dir.path().join("alphabet.rs"), "").unwrap();
        assert_eq!(search().hits.len(), 1);
        index.note_change(&to_canon(std::fs::canonicalize(dir.path()).unwrap()));
        assert_eq!(search().hits.len(), 2);
    }

    #[test]
    fn list_limits_are_not_falsely_truncated_at_an_exact_fit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        let index = FileIndexState::default();
        let root = to_canon(dir.path());
        let exact = fs_list_files(&index, root.clone(), Some(2), None, None, None).unwrap();
        assert_eq!(exact.files, ["a.rs", "b.rs"]);
        assert!(!exact.truncated);
        assert!(
            fs_list_files(&index, root, Some(1), None, None, None)
                .unwrap()
                .truncated
        );
    }

    #[test]
    fn visibility_and_depth_never_reuse_the_wrong_listing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let root = to_canon(dir.path());
        let index = FileIndexState::default();
        assert!(
            fs_list_files(&index, root.clone(), None, Some(1), None, None)
                .unwrap()
                .files
                .is_empty()
        );
        assert_eq!(
            fs_list_files(&index, root.clone(), None, None, None, None)
                .unwrap()
                .files,
            ["src/a.rs"]
        );
        assert!(fs_list_files(&index, root, None, None, None, Some(true))
            .unwrap()
            .files
            .contains(&".hidden".to_string()));
    }

    #[test]
    fn empty_or_zero_limit_searches_do_not_walk() {
        let index = FileIndexState::default();
        assert!(
            fs_search(&index, "/missing".into(), "".into(), None, None, None)
                .unwrap()
                .hits
                .is_empty()
        );
        assert!(fs_search(
            &index,
            "/missing".into(),
            "query".into(),
            Some(0),
            None,
            None
        )
        .unwrap()
        .hits
        .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_remain_searchable_but_are_not_listed_as_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::os::unix::fs::symlink("a.rs", dir.path().join("linked.rs")).unwrap();
        let root = to_canon(dir.path());
        let index = FileIndexState::default();
        assert_eq!(
            fs_list_files(&index, root.clone(), None, None, None, None)
                .unwrap()
                .files,
            ["a.rs"]
        );
        assert_eq!(
            fs_search(&index, root, "linked".into(), None, None, None)
                .unwrap()
                .hits
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_cache_keys_do_not_change_the_callers_display_path() {
        let dir = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let alias = parent.path().join("alias");
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        let index = FileIndexState::default();
        fs_search(&index, to_canon(dir.path()), "a".into(), None, None, None).unwrap();
        let result = fs_search(&index, to_canon(&alias), "a".into(), None, None, None).unwrap();
        assert_eq!(result.hits[0].path, to_canon(alias.join("a.rs")));
    }

    fn ranked(cands: Vec<SearchHit>, query: &str, cap: usize) -> Vec<SearchHit> {
        let mut packed = Packed::default();
        for cand in &cands {
            assert!(packed.push(&cand.rel, EntryKind::File));
        }
        rank_fuzzy(&packed, query, cap)
            .into_iter()
            .map(|i| cands[i].clone())
            .collect()
    }

    fn hit(rel: &str) -> SearchHit {
        SearchHit {
            path: rel.to_string(),
            rel: rel.to_string(),
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
            is_dir: false,
        }
    }

    #[test]
    fn rank_fuzzy_prefers_name_and_shorter_path() {
        let cands = vec![
            hit("src/deeply/nested/config.rs"),
            hit("config.rs"),
            hit("src/main.rs"),
        ];
        let out = ranked(cands, "config", 10);
        assert_eq!(out[0].rel, "config.rs");
        assert!(!out.iter().any(|h| h.rel == "src/main.rs"));
    }

    #[test]
    fn rank_fuzzy_matches_subsequence() {
        let cands = vec![hit("CommandPalette.tsx"), hit("readme.md")];
        let out = ranked(cands, "cmdp", 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rel, "CommandPalette.tsx");
    }
}
