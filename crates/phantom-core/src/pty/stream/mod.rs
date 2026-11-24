//! Shared libghostty PTY stream, adapted from Terax v0.9.0.

mod hub;
mod output;
mod presentation;
mod queue;
mod tokens;
mod wake;
pub mod wire;

use std::sync::Arc;

pub use hub::{Command, Endpoint, Hub, HubDiagnostics, Reply};
pub use output::InvalidBoundary;
pub use presentation::PresentationClass;
pub use queue::{Deadline, OutputQueue, QueueDiagnostics, Summary};
pub use tokens::StreamTokens;

struct CallbackReply<F>(F);

impl<F: FnOnce(u16, Vec<u8>) + Send> Reply for CallbackReply<F> {
    fn send(self: Box<Self>, status: u16, body: Vec<u8>) {
        (self.0)(status, body);
    }
}

impl Hub {
    pub fn request(
        &self,
        label: Arc<str>,
        endpoint: Endpoint,
        body: &[u8],
        reply: impl FnOnce(u16, Vec<u8>) + Send + 'static,
    ) {
        if body.len() > wire::MAX_REQUEST_LEN {
            reply(413, Vec::new());
            return;
        }
        let request = match wire::decode_request(body) {
            Ok(request) => request,
            Err(_) => {
                reply(400, Vec::new());
                return;
            }
        };
        if !self.tokens().authorizes(&label, request.token) {
            reply(403, Vec::new());
            return;
        }
        self.submit(Command::Request {
            label,
            endpoint,
            request,
            reply: Box::new(CallbackReply(reply)),
        });
    }
}
