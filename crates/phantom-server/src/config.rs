//! Server configuration.
//!
//! The file format deliberately mirrors code-server's `config.yaml` — same keys
//! for `bind-addr`, `auth`, `password`, `cert`, and `cert-key` — so an existing
//! code-server setup can be reused by copying the file, and the TLS certificate
//! can be shared rather than issued twice.
//!
//! Phantom adds one key code-server has no equivalent for: `username`. When it
//! is set the login form checks the name as well as the password; when it is
//! absent (a reused code-server file) password-only login is unchanged.
//!
//! Resolution order, first hit wins per field: environment variable → Phantom's
//! own config → code-server's config (as a convenience fallback) → default.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Mirrors the subset of code-server's config we understand. Unknown keys are
/// ignored so pointing this at a real code-server file doesn't fail.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigFile {
    bind_addr: Option<String>,
    auth: Option<String>,
    /// Phantom-only second login factor. Checked at the login form only when set.
    username: Option<String>,
    password: Option<String>,
    /// Argon2 PHC string. Takes precedence over `password` when both are set.
    hashed_password: Option<String>,
    cert: Option<PathBuf>,
    cert_key: Option<PathBuf>,
    /// Phantom-only: directory holding the built frontend.
    web_root: Option<PathBuf>,
    /// Phantom-only: `local` (a PTY this process owns) or `rmux` (the daemon).
    pty_backend: Option<String>,
    /// Phantom-only: the shared agent room. Absent means the room still works
    /// for browsers, but nothing outside this process can post into it.
    room: Option<RoomFile>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct RoomFile {
    name: Option<String>,
    /// This server's name in the room. Other machines address its agents as
    /// `<peer>/<agent>`, so it has to be unique across the mesh.
    peer: Option<String>,
    /// Bearer token per agent, for `POST /room/post`. One token per agent
    /// rather than one shared one: an agent's identity in the room *is* its
    /// token, so sharing would make everyone anonymous again.
    agent_tokens: Option<std::collections::BTreeMap<String, String>>,
    /// Hub side: token per peer server allowed to join, keyed by peer name.
    peer_tokens: Option<std::collections::BTreeMap<String, String>>,
    /// Token per remote client (a phone, a laptop, the desktop app), keyed by a
    /// name for the device. Grants the room and nothing else.
    client_tokens: Option<std::collections::BTreeMap<String, String>>,
    /// Peer side: where to dial. Setting this makes the server a peer rather
    /// than the hub — it forwards its posts upstream instead of owning a log.
    hub: Option<HubFile>,
    /// Workspace root the room reads handoff files from and writes fetched
    /// remote ones into. Without a browser there is no `cwd` to infer it from.
    workspace: Option<PathBuf>,
    /// Run as nothing but a room peer: no web UI, no editor, and the only
    /// listener is a loopback one for agents to post through.
    peer_only: Option<bool>,
    /// Start an agent from `workspace`'s `agents.yaml` when a room message
    /// mentions it and it isn't running. Off unless asked for: a mention from
    /// any machine in the room would otherwise start processes here, on this
    /// machine's subscriptions.
    auto_start: Option<bool>,
    /// A built-in assistant anyone in the room can ask, answered by a model
    /// rather than a CLI agent — for devices with no AI tools of their own.
    assistant: Option<AssistantFile>,
    /// Let the room look at this machine's agents' screens, read-only. Off
    /// unless asked for: a terminal can show code, file contents, secrets.
    peek: Option<bool>,
    /// The mesh's certificate authority. Every Phantom host in a mesh that has
    /// one presents a certificate signed by it, so this is the single thing
    /// each machine has to trust.
    ca: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct AssistantFile {
    /// What it is mentioned as, e.g. `ai` for `@ai`.
    name: String,
    /// The model to ask. When omitted, the `orchestrator:` block of
    /// `workspace`'s `agents.yaml` names it.
    model: Option<String>,
    /// An OpenAI-compatible chat-completions URL; GLM's when omitted.
    endpoint: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct HubFile {
    url: String,
    token: String,
    /// Certificate to trust when dialing the hub over `wss://`. Needed when the
    /// hub serves the self-signed cert `gen-phantom-cert.sh` produces, which no
    /// system root chains to.
    ca: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Every request must present a valid session cookie. `username` is part of
    /// the check only when set; a reused code-server config has no `username:`
    /// key, so leaving it unset keeps password-only login working.
    Password {
        username: Option<String>,
        password: String,
    },
    /// No check at all. Only sane on loopback.
    None,
}

#[derive(Debug, Clone)]
pub struct Tls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// Which process owns the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyBackend {
    /// A PTY owned by this process. Simple, but terminals die with the server.
    Local,
    /// The rmux daemon. Terminals outlive a restart and can be re-attached.
    Rmux,
}

/// The room's server-side settings. Tokens live here and only here — they are
/// never handed to a browser, which authenticates with the session cookie.
#[derive(Debug, Clone)]
pub struct Room {
    pub name: String,
    pub peer: String,
    /// Where an agent on this host posts back to. Derived from the bind address
    /// rather than configured, so it cannot drift from where the server listens.
    pub post_url: String,
    pub agent_tokens: std::collections::BTreeMap<String, String>,
    pub peer_tokens: std::collections::BTreeMap<String, String>,
    /// Room-only credentials. Deliberately separate from the login password,
    /// which reaches a shell through `/ws`: a device that just wants the chat
    /// must not be handed the key to the machine.
    pub client_tokens: std::collections::BTreeMap<String, String>,
    pub hub: Option<Hub>,
    pub workspace: Option<PathBuf>,
    /// Serve nothing but the room. The machine hosts agents for someone else's
    /// room and has no use for the web UI — see [`Config::load`], which also
    /// drops the password requirement and pins the listener to loopback.
    pub peer_only: bool,
    /// Start a mentioned agent that isn't running, rather than report that it
    /// isn't. Needs `workspace`, where the agents are declared.
    pub auto_start: bool,
    /// The room's built-in assistant, if this machine runs one.
    pub assistant: Option<Assistant>,
    /// Whether anyone in the room may see this machine's agents' screens.
    /// Someone logged in to this machine's own web UI always may: they can
    /// open the terminal itself.
    pub peek: bool,
    /// The mesh's CA, trusted by everything this process dials: `say` posting
    /// to this host, and the peer link when `hub.ca` does not override it.
    pub ca: Option<PathBuf>,
}

/// A model that answers in the room under an agent's name. The key is the
/// orchestrator's — the `orchestrator/apiKey` secret, else
/// `PHANTOM_ORCHESTRATOR_KEY` — so a machine that already has a brain set up
/// needs nothing new but a name.
#[derive(Debug, Clone)]
pub struct Assistant {
    pub name: String,
    pub model: Option<String>,
    pub endpoint: Option<String>,
}

/// Where a peer server dials to join someone else's room.
#[derive(Debug, Clone)]
pub struct Hub {
    pub url: String,
    pub token: String,
    pub ca: Option<PathBuf>,
}

impl Default for Room {
    fn default() -> Self {
        Self {
            name: "main".into(),
            peer: crate::room::LOCAL_PEER.into(),
            post_url: "http://127.0.0.1:8099".into(),
            agent_tokens: Default::default(),
            peer_tokens: Default::default(),
            client_tokens: Default::default(),
            hub: None,
            workspace: None,
            peer_only: false,
            auto_start: false,
            assistant: None,
            peek: false,
            ca: None,
        }
    }
}

impl Room {
    /// Tokens short enough to guess, each described for a startup warning.
    /// A token is the whole of what stands in front of what it opens — a
    /// shell-less room, yes, but also every agent the room can reach.
    pub fn weak_tokens(&self) -> Vec<String> {
        const MIN_LEN: usize = 16;
        let tables = [
            ("agent", &self.agent_tokens),
            ("peer", &self.peer_tokens),
            ("client", &self.client_tokens),
        ];
        let mut weak: Vec<String> = tables
            .into_iter()
            .flat_map(|(kind, table)| {
                table
                    .iter()
                    .filter(|(_, token)| token.len() < MIN_LEN)
                    .map(move |(name, _)| format!("the {kind} token for {name:?}"))
            })
            .collect();
        if self.hub.as_ref().is_some_and(|hub| hub.token.len() < MIN_LEN) {
            weak.push("the token for joining the hub".to_string());
        }
        weak
    }

    /// Which peer a bearer token belongs to. Same constant-time shape as
    /// [`Self::agent_for_token`], and for the same reason.
    pub fn peer_for_token(&self, token: &str) -> Option<&str> {
        constant_time_lookup(&self.peer_tokens, token)
    }

    /// Which client device a bearer token belongs to. Constant-time, as above.
    pub fn client_for_token(&self, token: &str) -> Option<&str> {
        constant_time_lookup(&self.client_tokens, token)
    }

    /// Which agent a bearer token belongs to, in constant time.
    ///
    /// Compares against every configured token rather than stopping at the
    /// first match, so response timing can't be used to learn how many agents
    /// are configured or to recover a token prefix.
    pub fn agent_for_token(&self, token: &str) -> Option<&str> {
        constant_time_lookup(&self.agent_tokens, token)
    }
}

fn constant_time_lookup<'a>(
    table: &'a std::collections::BTreeMap<String, String>,
    token: &str,
) -> Option<&'a str> {
    use subtle::ConstantTimeEq;
    let mut found = None;
    for (name, expected) in table {
        let hit: bool = expected.as_bytes().ct_eq(token.as_bytes()).into();
        if hit {
            found = Some(name.as_str());
        }
    }
    found
}

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub auth: Auth,
    pub tls: Option<Tls>,
    pub web_root: PathBuf,
    pub pty_backend: PtyBackend,
    pub room: Room,
}

impl ConfigFile {
    /// Field-wise merge: `self` wins, `other` fills the gaps.
    fn or(self, other: ConfigFile) -> ConfigFile {
        ConfigFile {
            bind_addr: self.bind_addr.or(other.bind_addr),
            auth: self.auth.or(other.auth),
            username: self.username.or(other.username),
            password: self.password.or(other.password),
            hashed_password: self.hashed_password.or(other.hashed_password),
            cert: self.cert.or(other.cert),
            cert_key: self.cert_key.or(other.cert_key),
            web_root: self.web_root.or(other.web_root),
            pty_backend: self.pty_backend.or(other.pty_backend),
            room: self.room.or(other.room),
        }
    }
}

fn phantom_config_path() -> Option<PathBuf> {
    std::env::var_os("PHANTOM_CONFIG")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|d| d.join("phantom/config.yaml")))
}

fn code_server_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("code-server/config.yaml"))
}

fn load(path: &Path) -> Option<ConfigFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_yaml::from_str::<ConfigFile>(&raw) {
        Ok(cfg) => {
            log::info!("loaded config from {}", path.display());
            Some(cfg)
        }
        Err(e) => {
            // Don't silently fall through to defaults: a typo'd config that
            // disables auth without saying so is exactly the failure we can't
            // afford here.
            log::error!("ignoring malformed config {}: {e}", path.display());
            None
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        // Merge field by field rather than picking one file: the common setup
        // is a short Phantom config (its own port) that inherits the password
        // and TLS certificate already configured for code-server, so the secret
        // and the cert path each live in exactly one place.
        let own = phantom_config_path().as_deref().and_then(load);
        let inherited = code_server_config_path().as_deref().and_then(load);
        let file = match (own, inherited) {
            (Some(own), Some(inherited)) => {
                let merged = own.or(inherited);
                log::info!("inheriting unset fields from code-server's config");
                merged
            }
            (Some(own), None) => own,
            (None, Some(inherited)) => {
                log::info!("no Phantom config found; using code-server's");
                inherited
            }
            (None, None) => ConfigFile::default(),
        };

        let host = std::env::var("PHANTOM_HOST").ok();
        let port = std::env::var("PHANTOM_PORT").ok();
        let addr = match (host, port, file.bind_addr.as_deref()) {
            // An explicit env var always wins, even over a bind-addr in a file.
            (Some(h), p, _) => format!("{h}:{}", p.unwrap_or_else(|| "8099".into())),
            (None, Some(p), _) => format!("127.0.0.1:{p}"),
            (None, None, Some(bind)) => bind.to_string(),
            (None, None, None) => "127.0.0.1:8099".to_string(),
        };
        let addr: SocketAddr = normalize_bind_addr(&addr)
            .parse()
            .map_err(|e| format!("invalid bind address {addr:?}: {e}"))?;

        // Read ahead of the auth check: a peer-only server has no login form, so
        // demanding a password for one would be a setup step that protects
        // nothing. Its only listener is on loopback and its endpoints carry
        // their own bearer tokens.
        let peer_only = file
            .room
            .as_ref()
            .and_then(|r| r.peer_only)
            .unwrap_or(false);

        let auth = match file.auth.as_deref() {
            Some("none") => Auth::None,
            _ if peer_only => Auth::None,
            // `password` is the default in code-server, so treat a missing
            // `auth:` key the same way rather than silently opening up.
            _ => {
                let password = std::env::var("PHANTOM_PASSWORD")
                    .ok()
                    .or(file.hashed_password)
                    .or(file.password)
                    .ok_or_else(|| {
                        "password auth is on but no password is set. Add `password:` to the \
                         config, set PHANTOM_PASSWORD, or set `auth: none` to disable auth"
                            .to_string()
                    })?;
                // An empty `username:` is treated as unset so a stray key in
                // the file cannot accidentally require a blank name.
                let username = std::env::var("PHANTOM_USERNAME")
                    .ok()
                    .or(file.username)
                    .filter(|u| !u.is_empty());
                Auth::Password { username, password }
            }
        };

        let cert = std::env::var_os("PHANTOM_CERT")
            .map(PathBuf::from)
            .or(file.cert);
        let key = std::env::var_os("PHANTOM_CERT_KEY")
            .map(PathBuf::from)
            .or(file.cert_key);
        let tls = match (cert, key) {
            (Some(cert), Some(key)) => Some(Tls { cert, key }),
            (Some(_), None) | (None, Some(_)) => {
                return Err("cert and cert-key must be set together".to_string())
            }
            (None, None) => None,
        };

        let web_root = std::env::var_os("PHANTOM_WEB_ROOT")
            .map(PathBuf::from)
            .or(file.web_root)
            .unwrap_or_else(|| PathBuf::from("dist"));

        let pty_backend = match std::env::var("PHANTOM_PTY_BACKEND")
            .ok()
            .or(file.pty_backend)
            .as_deref()
        {
            Some("rmux") => PtyBackend::Rmux,
            Some("local") | None => PtyBackend::Local,
            Some(other) => {
                return Err(format!(
                    "unknown pty-backend {other:?}; expected \"local\" or \"rmux\""
                ))
            }
        };

        let rf = file.room.unwrap_or_default();
        let room = Room {
            name: rf.name.unwrap_or_else(|| "main".into()),
            peer: rf.peer.unwrap_or_else(|| crate::room::LOCAL_PEER.into()),
            post_url: if peer_only {
                format!("http://127.0.0.1:{}", addr.port())
            } else {
                format!("{}://{addr}", if tls.is_some() { "https" } else { "http" })
            },
            agent_tokens: rf.agent_tokens.unwrap_or_default(),
            peer_tokens: rf.peer_tokens.unwrap_or_default(),
            client_tokens: rf.client_tokens.unwrap_or_default(),
            hub: rf.hub.map(|h| Hub { url: h.url, token: h.token, ca: h.ca }),
            workspace: rf.workspace,
            peer_only,
            auto_start: rf.auto_start.unwrap_or(false),
            peek: rf.peek.unwrap_or(false),
            assistant: match rf.assistant {
                // Mentioned as `@name`, so it has to parse as one.
                Some(a) if !crate::room::is_plain_name(&a.name) || a.name.contains('@') => {
                    return Err(format!(
                        "room.assistant.name {:?} must be a plain name, like `ai`",
                        a.name
                    ))
                }
                Some(a) => Some(Assistant { name: a.name, model: a.model, endpoint: a.endpoint }),
                None => None,
            },
            ca: rf.ca,
        };

        Ok(Self { addr, auth, tls, web_root, pty_backend, room })
    }
}

/// code-server writes `[::]:9888` for "all interfaces"; Rust wants `[::]:9888`
/// too, but a bare `0.0.0.0:port` or `host:port` also has to parse. Only the
/// unspecified-IPv6 shorthand needs help.
fn normalize_bind_addr(addr: &str) -> String {
    addr.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> ConfigFile {
        serde_yaml::from_str(yaml).expect("parse")
    }

    #[test]
    fn reads_code_servers_key_names() {
        let cfg = parse(
            r#"
bind-addr: "[::]:9888"
auth: password
password: hunter2
cert: /etc/letsencrypt/live/example/fullchain.pem
cert-key: /etc/letsencrypt/live/example/privkey.pem
"#,
        );
        assert_eq!(cfg.bind_addr.as_deref(), Some("[::]:9888"));
        assert_eq!(cfg.auth.as_deref(), Some("password"));
        assert_eq!(cfg.password.as_deref(), Some("hunter2"));
        assert!(cfg.cert.is_some() && cfg.cert_key.is_some());
    }

    #[test]
    fn reads_phantoms_username_key() {
        let cfg = parse("auth: password\nusername: operator\npassword: hunter2\n");
        assert_eq!(cfg.username.as_deref(), Some("operator"));
        assert_eq!(cfg.password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn username_is_optional() {
        let cfg = parse("auth: password\npassword: hunter2\n");
        assert_eq!(cfg.username, None);
    }

    #[test]
    fn ignores_keys_meant_for_code_server() {
        // A real code-server file carries settings we have no equivalent for;
        // reusing it must not fail.
        let cfg = parse("user-data-dir: /usr/local/project\nauth: password\npassword: x\n");
        assert_eq!(cfg.password.as_deref(), Some("x"));
    }

    #[test]
    fn reads_the_room_block() {
        let cfg = parse(
            "auth: none\nroom:\n  name: lab\n  peer: alpha\n  agent-tokens:\n    claude: tok-a\n",
        );
        let room = cfg.room.expect("room block");
        assert_eq!(room.name.as_deref(), Some("lab"));
        assert_eq!(room.peer.as_deref(), Some("alpha"));
        assert_eq!(room.agent_tokens.unwrap().get("claude").unwrap(), "tok-a");
        // Off unless a machine's owner turns it on.
        assert_eq!(room.auto_start, None);
        assert_eq!(parse("room:\n  auto-start: true\n").room.unwrap().auto_start, Some(true));
        let a = parse("room:\n  assistant:\n    name: ai\n    model: glm-4.6\n")
            .room
            .unwrap()
            .assistant
            .unwrap();
        assert_eq!((a.name.as_str(), a.model.as_deref()), ("ai", Some("glm-4.6")));
    }

    #[test]
    fn peer_only_needs_no_password() {
        // The trap this avoids: a peer box refusing to start until you invent a
        // password for a login form it never serves.
        let dir = std::env::temp_dir().join(format!("phantom-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("peer-only.yaml");
        std::fs::write(
            &path,
            "room:\n  peer-only: true\n  peer: b\n  hub:\n    url: wss://h/room/ws\n    token: t\n",
        )
        .unwrap();
        std::env::set_var("PHANTOM_CONFIG", &path);
        let cfg = Config::load().expect("peer-only config loads without a password");
        std::env::remove_var("PHANTOM_CONFIG");
        assert!(cfg.room.peer_only);
        assert_eq!(cfg.auth, Auth::None);
        // Agents post to this box's own loopback listener, never to the address
        // a full server would have advertised.
        assert!(cfg.room.post_url.starts_with("http://127.0.0.1:"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_the_mesh_ca() {
        let cfg = parse("auth: none\nroom:\n  ca: /etc/phantom/ca.pem\n");
        assert_eq!(
            cfg.room.unwrap().ca.unwrap().to_str(),
            Some("/etc/phantom/ca.pem")
        );
    }

    #[test]
    fn token_resolves_to_its_agent_and_nothing_else() {
        let mut agent_tokens = std::collections::BTreeMap::new();
        agent_tokens.insert("claude".to_string(), "tok-a".to_string());
        agent_tokens.insert("agy".to_string(), "tok-b".to_string());
        let room = Room { agent_tokens, ..Room::default() };
        // Short enough to guess, every one of them.
        assert_eq!(room.weak_tokens().len(), room.agent_tokens.len());
        let strong = Room {
            client_tokens: [("phone".to_string(), "0123456789abcdef0123".to_string())].into(),
            ..Room::default()
        };
        assert!(strong.weak_tokens().is_empty());
        assert_eq!(room.agent_for_token("tok-b"), Some("agy"));
        assert_eq!(room.agent_for_token("tok-c"), None);
        // A prefix of a real token must not authenticate.
        assert_eq!(room.agent_for_token("tok"), None);
        assert_eq!(room.agent_for_token(""), None);
    }

    #[test]
    fn bind_addr_forms_parse() {
        for addr in ["127.0.0.1:8099", "[::]:9888", "0.0.0.0:80", "192.0.2.5:8099"] {
            assert!(
                normalize_bind_addr(addr).parse::<SocketAddr>().is_ok(),
                "{addr} should parse"
            );
        }
    }
}
