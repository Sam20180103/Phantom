use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::redirect;

const IDLE_TTL: Duration = Duration::from_secs(90);
const MAX_CLIENTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Profile {
    Stream,
    Ping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientKey {
    profile: Profile,
    host: String,
    ips: Vec<IpAddr>,
    allow_private: bool,
}

impl ClientKey {
    fn new(profile: Profile, host: &str, ips: &[IpAddr], allow_private: bool) -> Self {
        let mut ips = ips.to_vec();
        ips.sort_unstable();
        ips.dedup();
        Self {
            profile,
            host: host.to_ascii_lowercase(),
            ips,
            allow_private,
        }
    }
}

struct Entry<C> {
    key: ClientKey,
    client: C,
    last_used: Instant,
}

struct ClientCache<C> {
    entries: Vec<Entry<C>>,
}

impl<C> Default for ClientCache<C> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<C: Clone> ClientCache<C> {
    fn get_or_insert_with(
        &mut self,
        key: ClientKey,
        now: Instant,
        make: impl FnOnce() -> Result<C, String>,
    ) -> Result<C, String> {
        self.entries
            .retain(|entry| now.saturating_duration_since(entry.last_used) < IDLE_TTL);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.last_used = now;
            return Ok(entry.client.clone());
        }
        let client = make()?;
        if self.entries.len() >= MAX_CLIENTS {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.entries.swap_remove(oldest);
        }
        self.entries.push(Entry {
            key,
            client: client.clone(),
            last_used: now,
        });
        Ok(client)
    }
}

static CLIENTS: OnceLock<Mutex<ClientCache<reqwest::Client>>> = OnceLock::new();

fn build_client(key: &ClientKey) -> Result<reqwest::Client, String> {
    let addresses: Vec<_> = key.ips.iter().map(|ip| SocketAddr::new(*ip, 0)).collect();
    let builder = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .resolve_to_addrs(&key.host, &addresses)
        .pool_idle_timeout(IDLE_TTL)
        .pool_max_idle_per_host(2);
    let builder = match key.profile {
        Profile::Ping => builder
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none()),
        Profile::Stream => {
            let host = key.host.clone();
            let allow_private = key.allow_private;
            builder
                .http2_initial_stream_window_size(256 * 1024)
                .http2_initial_connection_window_size(2 * 1024 * 1024)
                .redirect(reqwest::redirect::Policy::custom(
                    move |attempt| match redirect::decision(
                        attempt.url(),
                        std::slice::from_ref(&host),
                        attempt.previous().len(),
                        allow_private,
                    ) {
                        redirect::Decision::Follow => attempt.follow(),
                        redirect::Decision::Stop => attempt.stop(),
                        redirect::Decision::TooMany => attempt.error("too many redirects"),
                    },
                ))
        }
    };
    builder.build().map_err(|error| error.to_string())
}

pub(super) fn client_for(
    profile: Profile,
    host: &str,
    ips: &[IpAddr],
    allow_private: bool,
) -> Result<reqwest::Client, String> {
    if ips.is_empty() {
        return Err("cannot build AI client without validated addresses".into());
    }
    let key = ClientKey::new(profile, host, ips, allow_private);
    let cache = CLIENTS.get_or_init(|| Mutex::new(ClientCache::default()));
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    cache.get_or_insert_with(key.clone(), Instant::now(), || build_client(&key))
}

#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod tests;
