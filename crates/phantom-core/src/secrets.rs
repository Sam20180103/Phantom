//! Secret storage backed by a mode-0600 JSON file.
//!
//! This is the Linux behaviour the desktop app already had, lifted out so the
//! headless server shares it. The `keyring` crate's Linux backend is the Secret
//! Service over D-Bus, which silently fails without gnome-keyring/kwallet — on
//! a headless box there is no such daemon at all, so the file is not a fallback
//! here, it is the only option. User-only permissions provide the isolation the
//! secret-service collection would have given.
//!
//! macOS and Windows keep using `keyring` in the desktop build; that branch
//! stays in `src-tauri` since nothing else needs it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Secrets are addressed by (service, account); the file is flat.
pub fn key(service: &str, account: &str) -> String {
    format!("{}::{}", service, account)
}

pub fn read_store_at(path: &Path) -> Result<HashMap<String, String>, String> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice::<HashMap<String, String>>(&bytes).map_err(|e| e.to_string())
}

/// Writes via a temp file and a rename, so a crash mid-write cannot truncate
/// the store and lose every key the user has.
pub fn write_store_at(path: &Path, map: &HashMap<String, String>) -> Result<(), String> {
    use std::io::Write;

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(map).map_err(|e| e.to_string())?;
    let mut f = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| e.to_string())?;
    f.write_all(&bytes).map_err(|e| e.to_string())?;
    f.as_file().sync_all().map_err(|e| e.to_string())?;
    f.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// A secrets file plus its in-memory copy.
///
/// Reads are served from the cache so the AI panel's cold-boot fan-out doesn't
/// hit the disk once per provider; writes go through to disk immediately,
/// because losing a key the user just entered is worse than the write cost.
pub struct SecretsStore {
    path: PathBuf,
    cache: Mutex<Option<HashMap<String, String>>>,
}

impl SecretsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path, cache: Mutex::new(None) }
    }

    /// `~/.local/share/phantom/secrets.json`, or the current directory if the
    /// platform will not name a data dir.
    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("phantom/secrets.json")
    }

    fn with<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut HashMap<String, String>) -> R,
    {
        let mut guard = self.cache.lock().map_err(|e| e.to_string())?;
        if guard.is_none() {
            *guard = Some(read_store_at(&self.path)?);
        }
        Ok(f(guard.as_mut().expect("cache filled above")))
    }

    fn update(&self, change: impl FnOnce(&mut HashMap<String, String>)) -> Result<(), String> {
        self.with(|map| {
            let mut next = map.clone();
            change(&mut next);
            write_store_at(&self.path, &next)?;
            *map = next;
            Ok(())
        })?
    }

    pub fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        let k = key(service, account);
        self.with(|m| m.get(&k).cloned())
    }

    pub fn get_all(&self, service: &str, accounts: &[String]) -> Result<Vec<Option<String>>, String> {
        self.with(|m| {
            accounts
                .iter()
                .map(|a| m.get(&key(service, a)).cloned())
                .collect()
        })
    }

    pub fn set(&self, service: &str, account: &str, password: String) -> Result<(), String> {
        let k = key(service, account);
        self.update(|m| {
            m.insert(k, password);
        })
    }

    pub fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        let k = key(service, account);
        self.update(|m| {
            m.remove(&k);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn key_format_is_service_double_colon_account() {
        assert_eq!(key("openai", "alice"), "openai::alice");
        assert_eq!(key("", ""), "::");
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(read_store_at(&tmp.path().join("nope.json")).unwrap().is_empty());
    }

    #[test]
    fn garbage_file_errors_rather_than_silently_losing_keys() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("secrets.json");
        fs::write(&p, b"not json").unwrap();
        assert!(read_store_at(&p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_is_user_only_readable() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("secrets.json");
        write_store_at(&p, &HashMap::new()).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("secrets.json");
        write_store_at(&p, &HashMap::new()).unwrap();
        assert!(!p.with_extension("json.tmp").exists());
    }

    #[test]
    fn store_round_trips_through_a_fresh_instance() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("phantom/secrets.json");

        let a = SecretsStore::new(p.clone());
        a.set("svc", "alice", "k1".into()).unwrap();
        a.set("svc", "bob", "k2".into()).unwrap();

        // A second instance proves the write reached disk, not just the cache.
        let b = SecretsStore::new(p);
        assert_eq!(b.get("svc", "alice").unwrap().as_deref(), Some("k1"));
        assert_eq!(
            b.get_all("svc", &["alice".into(), "bob".into(), "carol".into()]).unwrap(),
            vec![Some("k1".into()), Some("k2".into()), None]
        );
    }

    #[test]
    fn delete_removes_only_its_own_key() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("secrets.json");
        let s = SecretsStore::new(p.clone());
        s.set("svc", "alice", "k1".into()).unwrap();
        s.set("svc", "bob", "k2".into()).unwrap();

        s.delete("svc", "alice").unwrap();

        let fresh = SecretsStore::new(p);
        assert_eq!(fresh.get("svc", "alice").unwrap(), None);
        assert_eq!(fresh.get("svc", "bob").unwrap().as_deref(), Some("k2"));
    }

    #[test]
    fn deleting_a_missing_key_is_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let s = SecretsStore::new(tmp.path().join("secrets.json"));
        assert!(s.delete("svc", "nobody").is_ok());
    }

    #[test]
    fn failed_writes_preserve_the_cached_keys() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("store/secrets.json");
        let store = SecretsStore::new(path.clone());
        store.set("svc", "alice", "saved".into()).unwrap();
        fs::rename(path.parent().unwrap(), tmp.path().join("saved")).unwrap();
        fs::write(path.parent().unwrap(), b"blocked").unwrap();

        assert!(store.set("svc", "alice", "unsaved".into()).is_err());
        assert!(store.delete("svc", "alice").is_err());
        assert_eq!(store.get("svc", "alice").unwrap().as_deref(), Some("saved"));
        assert_eq!(
            read_store_at(&tmp.path().join("saved/secrets.json")).unwrap()["svc::alice"],
            "saved"
        );
    }

    #[test]
    fn concurrent_updates_reach_disk_without_losing_keys() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secrets.json");
        let store = SecretsStore::new(path.clone());
        std::thread::scope(|scope| {
            for i in 0..16 {
                let store = &store;
                scope.spawn(move || store.set("svc", &i.to_string(), i.to_string()).unwrap());
            }
        });
        let saved = read_store_at(&path).unwrap();
        assert_eq!(saved.len(), 16);
        for i in 0..16 {
            assert_eq!(saved[&key("svc", &i.to_string())], i.to_string());
        }
    }

    #[cfg(unix)]
    #[test]
    fn stale_temporary_symlinks_are_never_followed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secrets.json");
        let victim = tmp.path().join("other.txt");
        fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, path.with_extension("json.tmp")).unwrap();
        write_store_at(&path, &HashMap::new()).unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert!(read_store_at(&path).unwrap().is_empty());
    }
}
