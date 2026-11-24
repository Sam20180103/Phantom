use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use phantom_core::pty::stream::{Command, Endpoint, Hub, OutputQueue, PresentationClass};
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::protocol::encode_stream_response;
use crate::session::Outbound;

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
static NEXT_PTY: AtomicU32 = AtomicU32::new(1);

pub struct PtyStream {
    hub: Hub,
    label: Arc<str>,
    active: AtomicBool,
    closed: AtomicBool,
    queues: Mutex<HashMap<u32, Arc<OutputQueue>>>,
    replies: Arc<Semaphore>,
}

impl PtyStream {
    pub fn new(hub: Hub) -> Self {
        Self {
            hub,
            label: Arc::from(format!(
                "socket-{}",
                NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)
            )),
            active: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            queues: Mutex::new(HashMap::new()),
            replies: Arc::new(Semaphore::new(4)),
        }
    }

    pub fn next_id(&self) -> u32 {
        NEXT_PTY.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn open(&self) -> Result<String, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("connection closed".into());
        }
        let (tx, rx) = oneshot::channel();
        self.hub.submit(Command::OpenStream {
            label: self.label.clone(),
            reply: Box::new(move |result| {
                let _ = tx.send(result);
            }),
        });
        let token = rx
            .await
            .map_err(|_| "PTY stream stopped")?
            .map_err(str::to_string)?;
        if self.closed.load(Ordering::Acquire) {
            self.hub.submit(Command::RemoveWebview {
                label: self.label.clone(),
            });
            return Err("connection closed".into());
        }
        self.active.store(true, Ordering::Release);
        Ok(format!("{token:016x}"))
    }

    pub fn register(
        &self,
        id: u32,
        class: PresentationClass,
    ) -> Result<Option<Arc<OutputQueue>>, String> {
        let mut queues = self.queues.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return Err("connection closed".into());
        }
        if !self.active.load(Ordering::Acquire) {
            return Ok(None);
        }
        let queue = self.hub.queue();
        self.hub.submit(Command::Register {
            id,
            label: self.label.clone(),
            queue: queue.clone(),
            class,
        });
        queues.insert(id, queue.clone());
        Ok(Some(queue))
    }

    pub fn request(
        &self,
        endpoint: Endpoint,
        req: u32,
        body: &[u8],
        tx: mpsc::UnboundedSender<Outbound>,
    ) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let Ok(permit) = self.replies.clone().try_acquire_owned() else {
            log::warn!("pty_stream: out of permits for endpoint={:?} req={}", endpoint, req);
            self.disconnect();
            let _ = tx.send(Outbound::Close);
            return;
        };
        self.hub
            .request(self.label.clone(), endpoint, body, move |status, body| {
                let _ = tx.send(Outbound::Stream(
                    encode_stream_response(req, status, &body),
                    permit,
                ));
            });
    }

    pub fn set_presentation(
        &self,
        ids: Vec<u32>,
        class: PresentationClass,
        seq: u64,
    ) -> Result<(), String> {
        if ids.len() > 4096 {
            return Err("too many terminals in presentation batch".into());
        }
        self.hub.submit(Command::SetPresentation {
            label: self.label.clone(),
            ids,
            class,
            seq,
        });
        Ok(())
    }

    pub fn close(&self, id: u32) {
        if let Some(queue) = self.queues.lock().unwrap().remove(&id) {
            queue.close();
            self.hub.submit(Command::Unregister { id });
        }
    }

    pub fn close_all(&self) {
        for (id, queue) in self.queues.lock().unwrap().drain() {
            queue.close();
            self.hub.submit(Command::Unregister { id });
        }
    }

    pub fn disconnect(&self) {
        self.closed.store(true, Ordering::Release);
        self.close_all();
        self.hub.submit(Command::RemoveWebview {
            label: self.label.clone(),
        });
    }
}

impl Drop for PtyStream {
    fn drop(&mut self) {
        self.disconnect();
    }
}
