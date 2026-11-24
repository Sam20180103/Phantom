use tauri::AppHandle;

#[tauri::command]
pub async fn fs_clipboard_paths(app: AppHandle) -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        let _ = app;
        tauri::async_runtime::spawn_blocking(platform::read_file_paths)
            .await
            .map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            #[cfg(target_os = "linux")]
            platform::request_file_paths(tx);
            #[cfg(not(target_os = "linux"))]
            let _ = tx.send(platform::read_file_paths());
        })
        .map_err(|error| error.to_string())?;
        tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(5))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|_| "clipboard read timed out or was cancelled".to_string())?
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use gtk::gdk;
    use phantom_core::fs::clipboard::{clipboard_file_paths, MAX_CLIPBOARD_BYTES};

    pub fn request_file_paths(tx: std::sync::mpsc::Sender<Result<Vec<String>, String>>) {
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        clipboard.request_uris(move |clipboard, uris| {
            if !uris.is_empty() {
                let _ = tx.send(clipboard_file_paths(uris.iter().map(|uri| uri.as_str())));
                return;
            }
            clipboard.request_contents(
                &gdk::Atom::intern("x-special/gnome-copied-files"),
                move |_, selection| {
                    if selection.length() <= 0 {
                        let _ = tx.send(Ok(Vec::new()));
                        return;
                    }
                    if selection.length() > MAX_CLIPBOARD_BYTES as i32 {
                        let _ = tx.send(Err("clipboard file list is too large".into()));
                        return;
                    }
                    let text = String::from_utf8_lossy(&selection.data()).into_owned();
                    let _ = tx.send(clipboard_file_paths(text.lines()));
                },
            );
        });
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
    use objc2_foundation::NSURL;
    use phantom_core::fs::clipboard::{
        clipboard_path, local_file_url, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_PATHS,
    };

    pub fn read_file_paths() -> Result<Vec<String>, String> {
        let pasteboard = NSPasteboard::generalPasteboard();
        let Some(items) = pasteboard.pasteboardItems() else {
            return Ok(Vec::new());
        };
        let file_type = unsafe { NSPasteboardTypeFileURL };
        let mut paths = Vec::new();
        let mut bytes = 0;
        for (index, item) in items.iter().enumerate() {
            if index >= MAX_CLIPBOARD_PATHS {
                return Err("clipboard has too many items".into());
            }
            let Some(raw) = item.stringForType(file_type) else {
                continue;
            };
            if raw.length() > 32768 {
                return Err("clipboard file URL is too long".into());
            }
            if local_file_url(&raw.to_string()).is_none() {
                continue;
            }
            let Some(url) = NSURL::URLWithString(&raw) else {
                continue;
            };
            let Some(path) = url.filePathURL().and_then(|url| url.path()) else {
                continue;
            };
            let path = clipboard_path(path.to_string())?;
            bytes += path.len();
            if bytes > MAX_CLIPBOARD_BYTES {
                return Err("clipboard file list is too large".into());
            }
            paths.push(path);
        }
        Ok(paths)
    }
}

#[cfg(windows)]
mod platform {
    use phantom_core::fs::clipboard::{clipboard_path, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_PATHS};
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    pub fn read_file_paths() -> Result<Vec<String>, String> {
        let mut opened = false;
        for _ in 0..5 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !opened {
            return Err("clipboard is busy; try pasting again".into());
        }
        let _guard = ClipboardGuard;
        let hdrop = unsafe { GetClipboardData(u32::from(CF_HDROP)) };
        if hdrop.is_null() {
            return Ok(Vec::new());
        }
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
        if count as usize > MAX_CLIPBOARD_PATHS {
            return Err("clipboard has too many items".into());
        }
        let mut paths = Vec::new();
        let mut bytes = 0;
        for index in 0..count {
            let len = unsafe { DragQueryFileW(hdrop, index, std::ptr::null_mut(), 0) };
            if len == 0 {
                continue;
            }
            if len > 32768 {
                return Err("clipboard path is too long".into());
            }
            let mut buf = vec![0u16; len as usize + 1];
            let written =
                unsafe { DragQueryFileW(hdrop, index, buf.as_mut_ptr(), buf.len() as u32) };
            buf.truncate(written as usize);
            if !buf.is_empty() {
                let path = clipboard_path(OsString::from_wide(&buf))?;
                bytes += path.len();
                if bytes > MAX_CLIPBOARD_BYTES {
                    return Err("clipboard file list is too large".into());
                }
                paths.push(path);
            }
        }
        Ok(paths)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    pub fn read_file_paths() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}
