//! Secret storage with platform-appropriate backends.
//!
//! - macOS: macOS Keychain (via `keyring` crate)
//! - Windows: Credential Manager (via `keyring` crate)
//! - Linux: a file in the app's local data dir, mode 0600, implemented in
//!   `phantom_core::secrets` so the headless server shares it. The default
//!   `keyring` backend on Linux is the Secret Service over D-Bus, which
//!   silently fails on systems without gnome-keyring/kwallet (and on the
//!   "login" collection not being created). For an open-source desktop
//!   app shipped via AppImage/deb/rpm, we cannot assume a keyring daemon
//!   exists. The file backend is the same approach Brave/Chromium fall
//!   back to in that scenario; user-only file permissions provide the
//!   isolation the secret-service collection would have otherwise.
//!
//! The frontend talks to `secrets_get`, `secrets_set`, `secrets_delete`,
//! and `secrets_get_all` — no platform branching in JS.
//!
//! All commands take `&AppHandle` so we can resolve the data directory
//! once via Tauri's path API.

use tauri::{AppHandle, Manager};

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[derive(Default)]
pub struct SecretsState {
    #[cfg(target_os = "linux")]
    store: OnceLock<phantom_core::secrets::SecretsStore>,
}

#[cfg(target_os = "linux")]
use phantom_core::secrets::SecretsStore;

#[cfg(target_os = "linux")]
fn with_store<F, R>(app: &AppHandle, state: &SecretsState, f: F) -> Result<R, String>
where
    F: FnOnce(&SecretsStore) -> Result<R, String>,
{
    let store = match state.store.get() {
        Some(store) => store,
        None => {
            let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
            state
                .store
                .get_or_init(|| SecretsStore::new(dir.join("secrets.json")))
        }
    };
    f(store)
}

#[cfg(not(target_os = "linux"))]
fn entry(service: &str, account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(service, account).map_err(|e| e.to_string())
}

async fn blocking_secret<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &SecretsState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<SecretsState>()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn secrets_get(
    app: AppHandle,
    service: String,
    account: String,
) -> Result<Option<String>, String> {
    blocking_secret(app, move |app, state| {
        get_secret(app, state, &service, &account)
    })
    .await
}

fn get_secret(
    app: &AppHandle,
    state: &SecretsState,
    service: &str,
    account: &str,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "linux")]
    {
        with_store(app, state, |store| store.get(service, account))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state);
        let e = entry(service, account)?;
        match e.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }
}

#[tauri::command]
pub async fn secrets_set(
    app: AppHandle,
    service: String,
    account: String,
    password: String,
) -> Result<(), String> {
    blocking_secret(app, move |app, state| {
        set_secret(app, state, &service, &account, password)
    })
    .await
}

fn set_secret(
    app: &AppHandle,
    state: &SecretsState,
    service: &str,
    account: &str,
    password: String,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        with_store(app, state, |store| store.set(service, account, password))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state);
        let e = entry(service, account)?;
        e.set_password(&password).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn secrets_delete(
    app: AppHandle,
    service: String,
    account: String,
) -> Result<(), String> {
    blocking_secret(app, move |app, state| {
        delete_secret(app, state, &service, &account)
    })
    .await
}

fn delete_secret(
    app: &AppHandle,
    state: &SecretsState,
    service: &str,
    account: &str,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        with_store(app, state, |store| store.delete(service, account))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state);
        let e = entry(service, account)?;
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// Batch read uses a single IPC roundtrip for the cold-boot fan-out.
#[tauri::command]
pub async fn secrets_get_all(
    app: AppHandle,
    service: String,
    accounts: Vec<String>,
) -> Result<Vec<Option<String>>, String> {
    blocking_secret(app, move |app, state| {
        get_all_secrets(app, state, &service, accounts)
    })
    .await
}

fn get_all_secrets(
    app: &AppHandle,
    state: &SecretsState,
    service: &str,
    accounts: Vec<String>,
) -> Result<Vec<Option<String>>, String> {
    #[cfg(target_os = "linux")]
    {
        with_store(app, state, |store| store.get_all(service, &accounts))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state);
        Ok(accounts
            .into_iter()
            .map(|a| {
                keyring::Entry::new(service, &a)
                    .ok()
                    .and_then(|e| e.get_password().ok())
            })
            .collect())
    }
}
