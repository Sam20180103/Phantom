use super::{publish::rename_no_replace, to_canon};
use crate::workspace::{resolve_path, WorkspaceEnv, WorkspaceRegistry};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

const MAX_COPY_SOURCES: usize = 1024;
const MAX_COPY_SUFFIX: usize = 10_000;
const MAX_COPY_DEPTH: usize = 128;

#[derive(Debug, Default, Serialize)]
pub struct CopyResult {
    pub created: Vec<String>,
    pub failed: Vec<CopyFailure>,
}

#[derive(Debug, Serialize)]
pub struct CopyFailure {
    pub source: String,
    pub message: String,
}

pub fn fs_copy(
    sources: Vec<String>,
    dest_dir: String,
    root: String,
    workspace: Option<WorkspaceEnv>,
    registry: &WorkspaceRegistry,
    rename_on_conflict: bool,
) -> Result<CopyResult, String> {
    if sources.len() > MAX_COPY_SOURCES {
        return Err(format!("copy at most {MAX_COPY_SOURCES} items at a time"));
    }
    let workspace = WorkspaceEnv::from_option(workspace);
    let dest = copy_destination(&dest_dir, &root, &workspace, registry)?;
    let mut result = CopyResult::default();
    for source in sources {
        match copy_item(Path::new(&source), &dest, rename_on_conflict) {
            Ok(name) => result
                .created
                .push(to_canon(Path::new(&dest_dir).join(name))),
            Err(error) => result.failed.push(CopyFailure {
                source,
                message: error.to_string(),
            }),
        }
    }
    Ok(result)
}

fn copy_destination(
    dest_dir: &str,
    root: &str,
    workspace: &WorkspaceEnv,
    registry: &WorkspaceRegistry,
) -> Result<PathBuf, String> {
    let root = resolve_path(root, workspace);
    let dest = resolve_path(dest_dir, workspace);
    if !root.is_absolute() || !dest.is_absolute() {
        return Err("copy destination and workspace root must be absolute".into());
    }
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let dest = std::fs::canonicalize(dest).map_err(|error| error.to_string())?;
    if !registry.is_authorized(&root) || !dest.starts_with(&root) {
        return Err("copy destination is outside the authorized workspace".into());
    }
    if !dest.is_dir() {
        return Err("copy destination is not a directory".into());
    }
    Ok(dest)
}

fn copy_item(src: &Path, dest: &Path, rename_on_conflict: bool) -> io::Result<OsString> {
    if !src.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy source must be absolute",
        ));
    }
    let name = src.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "copy source has no filename")
    })?;
    let metadata = std::fs::symlink_metadata(src)?;
    if !rename_on_conflict && std::fs::symlink_metadata(dest.join(name)).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    if metadata.is_dir() && dest.starts_with(std::fs::canonicalize(src)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot copy a folder into itself",
        ));
    }
    // Only complete entries are published; failures leave no visible partial copy.
    let staging = tempfile::Builder::new()
        .prefix(".phantom-copy-")
        .tempdir_in(dest)?;
    let staged = staging.path().join("item");
    copy_recursive(src, &staged, 0)?;
    publish_copy(&staged, dest, name, metadata.is_dir(), rename_on_conflict)
}

fn publish_copy(
    staged: &Path,
    dest: &Path,
    name: &OsStr,
    is_dir: bool,
    rename: bool,
) -> io::Result<OsString> {
    for n in 0..=MAX_COPY_SUFFIX {
        let name = if n == 0 {
            name.to_os_string()
        } else {
            copy_name(name, n, is_dir)
        };
        let target = dest.join(&name);
        match rename_no_replace(staged, &target) {
            Ok(()) => return Ok(name),
            Err(error) => {
                if std::fs::symlink_metadata(&target).is_err() {
                    return Err(error);
                }
                if !rename {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("already exists: {}", target.display()),
                    ));
                }
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free copy name",
    ))
}

fn copy_recursive(src: &Path, dst: &Path, depth: usize) -> io::Result<()> {
    if depth > MAX_COPY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "folder nesting exceeds the copy limit",
        ));
    }
    let metadata = std::fs::symlink_metadata(src)?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        copy_symlink(src, dst)
    } else if kind.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()), depth + 1)?;
        }
        Ok(())
    } else if kind.is_file() {
        let mut source = std::fs::File::open(src)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        io::copy(&mut source, &mut target)?;
        target.set_permissions(metadata.permissions())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only files, folders and symbolic links can be copied",
        ))
    }
}

#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
}

#[cfg(windows)]
fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file, FileTypeExt};
    let target = std::fs::read_link(src)?;
    if std::fs::symlink_metadata(src)?.file_type().is_symlink_dir() {
        symlink_dir(target, dst)
    } else {
        symlink_file(target, dst)
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_symlink(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copying symbolic links is unsupported",
    ))
}

fn copy_name(name: &OsStr, n: usize, is_dir: bool) -> OsString {
    let path = Path::new(name);
    let (stem, ext) = match (is_dir, path.file_stem(), path.extension()) {
        (false, Some(stem), Some(ext)) => (stem, Some(ext)),
        _ => (name, None),
    };
    let mut out = stem.to_os_string();
    out.push(if n == 1 {
        " copy".to_string()
    } else {
        format!(" copy {n}")
    });
    if let Some(ext) = ext {
        out.push(".");
        out.push(ext);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(sources: &[PathBuf], dest: &Path, rename: bool) -> CopyResult {
        let registry = WorkspaceRegistry::default();
        registry.authorize(dest).unwrap();
        fs_copy(
            sources.iter().map(to_canon).collect(),
            to_canon(dest),
            to_canon(dest),
            None,
            &registry,
            rename,
        )
        .unwrap()
    }

    #[test]
    fn copies_files_and_directories_without_removing_sources() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), b"payload").unwrap();
        std::fs::create_dir_all(src.path().join("d/inner")).unwrap();
        std::fs::write(src.path().join("d/inner/y.txt"), b"nested").unwrap();
        let result = copy(
            &[src.path().join("a.txt"), src.path().join("d")],
            dest.path(),
            false,
        );
        assert!(result.failed.is_empty());
        assert_eq!(
            result.created,
            [
                to_canon(dest.path().join("a.txt")),
                to_canon(dest.path().join("d"))
            ]
        );
        assert_eq!(
            std::fs::read(dest.path().join("a.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(
            std::fs::read(dest.path().join("d/inner/y.txt")).unwrap(),
            b"nested"
        );
        assert!(src.path().join("a.txt").exists());
    }

    #[test]
    fn names_files_dotfiles_and_dotted_directories() {
        assert_eq!(copy_name(OsStr::new("a.txt"), 1, false), "a copy.txt");
        assert_eq!(copy_name(OsStr::new("a.txt"), 2, false), "a copy 2.txt");
        assert_eq!(copy_name(OsStr::new(".env"), 1, false), ".env copy");
        assert_eq!(
            copy_name(OsStr::new("Makefile"), 3, false),
            "Makefile copy 3"
        );
        assert_eq!(copy_name(OsStr::new("v1.2"), 1, true), "v1.2 copy");
    }

    #[test]
    fn conflicts_never_replace_existing_files_or_directories() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), b"new").unwrap();
        std::fs::write(dest.path().join("a.txt"), b"old").unwrap();
        std::fs::create_dir(dest.path().join("a copy.txt")).unwrap();
        let result = copy(&[src.path().join("a.txt")], dest.path(), false);
        assert!(result.created.is_empty());
        assert_eq!(result.failed.len(), 1);
        let result = copy(&[src.path().join("a.txt")], dest.path(), true);
        assert!(result.failed.is_empty());
        assert_eq!(result.created, [to_canon(dest.path().join("a copy 2.txt"))]);
        assert_eq!(std::fs::read(dest.path().join("a.txt")).unwrap(), b"old");
        assert_eq!(
            std::fs::read(dest.path().join("a copy 2.txt")).unwrap(),
            b"new"
        );
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 3);
    }

    #[test]
    fn concurrent_copies_publish_distinct_names() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a"), b"source").unwrap();
        let barrier = std::sync::Barrier::new(4);
        let results = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        copy(&[src.path().join("a")], dest.path(), true)
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        let mut paths = std::collections::HashSet::new();
        for result in results {
            assert!(result.failed.is_empty());
            assert_eq!(result.created.len(), 1);
            assert!(paths.insert(result.created[0].clone()));
            assert_eq!(std::fs::read(&result.created[0]).unwrap(), b"source");
        }
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 4);
    }

    #[test]
    fn partial_failure_does_not_stop_the_rest_of_the_batch() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a"), b"a").unwrap();
        std::fs::write(src.path().join("b"), b"b").unwrap();
        let result = copy(
            &[
                src.path().join("a"),
                src.path().join("missing"),
                src.path().join("b"),
            ],
            dest.path(),
            true,
        );
        assert_eq!(result.created.len(), 2);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(
            result.failed[0].source,
            to_canon(src.path().join("missing"))
        );
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 2);
    }

    #[test]
    fn refuses_self_copy_but_allows_a_sibling_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("d");
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        for dest in [&folder, &folder.join("inner")] {
            let result = copy(std::slice::from_ref(&folder), dest, true);
            assert!(result.created.is_empty());
            assert!(result.failed[0].message.contains("into itself"));
        }
        let result = copy(&[folder], root.path(), true);
        assert!(result.failed.is_empty());
        assert!(root.path().join("d copy/inner").is_dir());
    }

    #[test]
    fn destination_requires_an_authorized_absolute_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let registry = WorkspaceRegistry::default();
        let env = WorkspaceEnv::Local;
        let path = to_canon(root.path());
        assert!(copy_destination(&path, &path, &env, &registry).is_err());
        registry.authorize(root.path()).unwrap();
        assert!(copy_destination(&to_canon(outside.path()), &path, &env, &registry).is_err());
        assert!(copy_destination("relative", &path, &env, &registry).is_err());
        assert!(copy_destination(&path, "relative", &env, &registry).is_err());
        assert!(copy_destination(&path, &path, &env, &registry).is_ok());
    }

    #[test]
    fn limits_source_count_and_rejects_relative_sources() {
        let dest = tempfile::tempdir().unwrap();
        let registry = WorkspaceRegistry::default();
        registry.authorize(dest.path()).unwrap();
        assert!(fs_copy(
            vec!["relative".into(); MAX_COPY_SOURCES + 1],
            to_canon(dest.path()),
            to_canon(dest.path()),
            None,
            &registry,
            false
        )
        .unwrap_err()
        .contains("at most"));
        let result = fs_copy(
            vec!["relative".into()],
            to_canon(dest.path()),
            to_canon(dest.path()),
            None,
            &registry,
            false,
        )
        .unwrap();
        assert!(result.created.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert!(result.failed[0].message.contains("absolute"));
    }

    #[test]
    fn depth_limit_leaves_no_partial_directory() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let folder = src.path().join("d");
        let mut path = folder.clone();
        for _ in 0..=MAX_COPY_DEPTH {
            path.push("d");
        }
        std::fs::create_dir_all(path).unwrap();
        let result = copy(&[folder], dest.path(), false);
        assert!(result.created.is_empty());
        assert!(result.failed[0].message.contains("nesting"));
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[test]
    fn result_serializes_the_frontend_contract() {
        let result = CopyResult {
            created: vec!["/a".into()],
            failed: vec![CopyFailure {
                source: "/b".into(),
                message: "missing".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({ "created": ["/a"], "failed": [{ "source": "/b", "message": "missing" }] })
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_links_without_recursing_or_following_conflicting_links() {
        use std::os::unix::fs::symlink;
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir(src.path().join("d")).unwrap();
        symlink(".", src.path().join("d/cycle")).unwrap();
        symlink("missing", src.path().join("d/dangling")).unwrap();
        let result = copy(&[src.path().join("d")], dest.path(), true);
        assert!(result.failed.is_empty());
        assert_eq!(
            std::fs::read_link(dest.path().join("d/cycle")).unwrap(),
            Path::new(".")
        );
        assert_eq!(
            std::fs::read_link(dest.path().join("d/dangling")).unwrap(),
            Path::new("missing")
        );
        std::fs::write(src.path().join("a"), b"new").unwrap();
        std::fs::write(src.path().join("victim"), b"keep").unwrap();
        symlink(src.path().join("victim"), dest.path().join("a")).unwrap();
        let result = copy(&[src.path().join("a")], dest.path(), true);
        assert!(result.failed.is_empty());
        assert_eq!(std::fs::read(src.path().join("victim")).unwrap(), b"keep");
        assert_eq!(std::fs::read(dest.path().join("a copy")).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_destination_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let registry = WorkspaceRegistry::default();
        registry.authorize(root.path()).unwrap();
        assert!(copy_destination(
            &to_canon(root.path().join("escape")),
            &to_canon(root.path()),
            &WorkspaceEnv::Local,
            &registry
        )
        .is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_nested_entries_leave_no_visible_partial_copy() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir(src.path().join("d")).unwrap();
        std::fs::write(src.path().join("d/a"), b"a").unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(src.path().join("d/socket")).unwrap();
        let result = copy(&[src.path().join("d")], dest.path(), true);
        assert!(result.created.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }
}
