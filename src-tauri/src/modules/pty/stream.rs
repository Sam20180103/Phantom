use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use phantom_core::pty::stream::{Command, Hub, OutputQueue, PresentationClass};
use tauri::Manager;

use super::gate::AllowedOrigins;
use super::PtyState;

type OwnedQueue = (Arc<str>, Arc<OutputQueue>);

#[derive(Default)]
pub struct NativePtyStream {
    pub(crate) hub: Hub,
    pub(super) origins: OnceLock<AllowedOrigins>,
    pub(super) opening: Mutex<()>,
    queues: Mutex<HashMap<u32, OwnedQueue>>,
}

impl NativePtyStream {
    pub fn register(&self, id: u32, label: &str, class: PresentationClass) -> Arc<OutputQueue> {
        let queue = self.hub.queue();
        let label = Arc::from(label);
        self.queues
            .lock()
            .unwrap()
            .insert(id, (Arc::clone(&label), queue.clone()));
        self.hub.submit(Command::Register {
            id,
            label,
            queue: queue.clone(),
            class,
        });
        queue
    }

    pub fn owns(&self, id: u32, label: &str) -> bool {
        self.queues
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|(owner, _)| owner.as_ref() == label)
    }

    pub fn close(&self, id: u32) {
        if let Some((_, queue)) = self.queues.lock().unwrap().remove(&id) {
            queue.close();
            self.hub.submit(Command::Unregister { id });
        }
    }

    pub fn close_webview(&self, label: &str, ptys: &PtyState) -> usize {
        let _opening = self.opening.lock().unwrap();
        let ids: Vec<_> = self
            .queues
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(&id, (owner, _))| (owner.as_ref() == label).then_some(id))
            .collect();
        let count = ids.len();
        for id in ids {
            self.close(id);
            let _ = ptys.close(id);
        }
        self.hub.submit(Command::RemoveWebview {
            label: Arc::from(label),
        });
        count
    }
}

#[tauri::command]
pub async fn pty_stream_open(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    state: tauri::State<'_, NativePtyStream>,
) -> Result<String, String> {
    let url = webview.url().map_err(|error| error.to_string())?;
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let origin = format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        port
    );
    let origins = state.origins.get_or_init(|| {
        AllowedOrigins::for_app(
            app.config().build.dev_url.as_ref(),
            tauri::is_dev(),
            cfg!(windows),
        )
    });
    if origins.check(Some(origin.as_bytes())).is_none() {
        return Err("PTY stream requires the application origin".into());
    }
    if webview.label() == "main" {
        let presentation =
            app.state::<crate::modules::window_presentation::WindowPresentationState>();
        presentation.publish(&state.hub);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    state.hub.submit(Command::OpenStream {
        label: Arc::from(webview.label()),
        reply: Box::new(move |result| {
            let _ = tx.send(result);
        }),
    });
    let token = rx
        .await
        .map_err(|_| "PTY stream stopped")?
        .map_err(str::to_string)?;
    Ok(format!("{token:016x}"))
}

#[tauri::command]
pub fn pty_set_presentation(
    webview: tauri::Webview,
    state: tauri::State<'_, NativePtyStream>,
    ids: Vec<u32>,
    class: PresentationClass,
    seq: u64,
) -> Result<(), String> {
    if ids.len() > 4096 {
        return Err("too many terminals in presentation batch".into());
    }
    state.hub.submit(Command::SetPresentation {
        label: Arc::from(webview.label()),
        ids,
        class,
        seq,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_and_close_are_scoped_to_the_webview() {
        let streams = NativePtyStream::default();
        let ptys = PtyState::default();
        let first = streams.register(1, "main", PresentationClass::Focused);
        let second = streams.register(2, "settings", PresentationClass::Hidden);
        assert!(streams.owns(1, "main"));
        assert!(!streams.owns(1, "settings"));
        streams.close_webview("main", &ptys);
        assert!(!streams.owns(1, "main"));
        assert!(streams.owns(2, "settings"));
        assert!(!first.push(b"closed"));
        assert!(second.push(b"open"));
        streams.close(2);
    }

    #[test]
    fn webview_close_waits_for_an_open_in_progress() {
        let streams = Arc::new(NativePtyStream::default());
        let opening = streams.opening.lock().unwrap();
        let queue = streams.register(1, "main", PresentationClass::Focused);
        let (started, ready) = std::sync::mpsc::channel();
        let closing = Arc::clone(&streams);
        let thread = std::thread::spawn(move || {
            started.send(()).unwrap();
            closing.close_webview("main", &PtyState::default())
        });
        ready.recv().unwrap();
        assert!(streams.owns(1, "main"));
        drop(opening);
        assert_eq!(thread.join().unwrap(), 1);
        assert!(!queue.push(b"closed"));
    }
}
