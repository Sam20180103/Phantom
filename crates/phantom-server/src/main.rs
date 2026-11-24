//! Phantom's headless gateway: serves the built frontend and bridges it to
//! `phantom-core` over a WebSocket.

mod auth;
mod blob;
mod config;
mod agent_commands;
mod ai_session_commands;
mod agents;
mod files;
mod fs_commands;
mod git_commands;
mod history_commands;
mod net_commands;
mod orchestrator;
mod protocol;
mod pty_stream;
mod rmux_pty;
mod rmux_queries;
mod room;
mod say;
mod session;
mod shell_commands;
mod ws;

use std::sync::Arc;

use axum::extract::Request;
use axum::http::header::{HeaderValue, CACHE_CONTROL};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use config::{Auth, Config};
use phantom_core::workspace::WorkspaceRegistry;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};

pub struct AppState {
    pub file_index: phantom_core::fs::index::FileIndexState,
    pub pty_stream: phantom_core::pty::stream::Hub,
    pub registry: Arc<WorkspaceRegistry>,
    /// Shared, not per-connection: an API key the user saves in one tab has to
    /// be there in the next one.
    pub secrets: Arc<phantom_core::secrets::SecretsStore>,
    /// Shared: the shell-history index is the same for every tab, and building
    /// it reads history files and scans PATH.
    pub history: Arc<phantom_core::history::HistoryState>,
    pub config: Config,
    pub sessions: auth::Sessions,
    /// What every agent session is doing. Server-wide and viewer-independent,
    /// so a `nudge` from the room, an HTTP post or a peer link sees the truth
    /// even with no terminal open anywhere.
    pub agents: Arc<rmux_pty::AgentTracker>,
    /// The shared message log every participant posts into.
    pub rooms: Arc<room::Rooms>,
    /// Used to inject room messages into agent sessions from paths that have no
    /// connection of their own — the HTTP post endpoint, and later a peer link.
    pub room_rmux: Arc<rmux_pty::RmuxState>,
}

#[tokio::main]
async fn main() {
    // reqwest (aws-lc-rs) and axum-server (ring) both pull in a rustls crypto
    // provider, so rustls can no longer auto-pick one. Choose ring explicitly,
    // matching the TLS-serving path; without this the server panics at startup.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // One subcommand, handled before anything is bound: `say` is a client, not
    // a server, and it exists so an agent can post into the room with a single
    // command instead of hand-rolling an HTTP call.
    // Before the logger, so the CLI's output is its own and nothing else.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("say") {
        if let Err(e) = say::run(&args.collect::<Vec<_>>()).await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };

    if !config.room.peer_only {
        warn_about_exposure(&config);
    }
    for weak in config.room.weak_tokens() {
        log::warn!("{weak} is short enough to guess; generate one with `openssl rand -hex 32`");
    }

    let addr = config.addr;
    let tls = config.tls.clone();
    let web_root = config.web_root.clone();

    // Same startup contract as the desktop app (`src-tauri/src/lib.rs`):
    // without it the registry starts empty, so every path-taking command fails
    // with "outside the authorized workspace" until the frontend happens to
    // authorize a folder. The terminal masked this, since it resolves its own
    // cwd through `user_spawn_cwd_or_home` rather than the registry.
    let registry = Arc::new(WorkspaceRegistry::default());
    phantom_core::workspace::bootstrap_registry(&registry);

    let agents = rmux_pty::AgentTracker::new();
    let rooms = Arc::new(room::Rooms::new(&config.room));

    let state = Arc::new(AppState {
        file_index: Default::default(),
        pty_stream: phantom_core::pty::stream::Hub::new(),
        registry,
        secrets: Arc::new(phantom_core::secrets::SecretsStore::new(
            phantom_core::secrets::SecretsStore::default_path(),
        )),
        history: Arc::new(phantom_core::history::HistoryState::default()),
        config,
        sessions: auth::Sessions::default(),
        room_rmux: Arc::new(rmux_pty::RmuxState::new(agents.clone(), rooms.clone())),
        agents: agents.clone(),
        rooms,
    });

    // Watch every agent session for hook markers, regardless of who is looking.
    tokio::spawn(agents.supervise());

    // Join someone else's room, if configured to; a hub instead keeps its
    // peers told who else is in the room.
    if state.config.room.hub.is_some() {
        tokio::spawn(room::peer::run_link(state.clone()));
    } else {
        tokio::spawn(room::peer::run_directory(state.clone()));
    }

    if state.config.room.peer_only {
        serve_peer_only(state).await;
        return;
    }

    // Unmatched paths fall back to index.html so client-side routes survive a
    // hard refresh.
    let index = web_root.join("index.html");
    let static_files = ServeDir::new(&web_root).not_found_service(ServeFile::new(index));

    // Everything except the login form and the health probe sits behind auth,
    // including the static assets: serving the app shell to an unauthenticated
    // client leaks nothing useful but invites confusion about what's protected.
    // `.layer`, not `.route_layer`: the latter runs only for requests that
    // match a declared route, so the static-file fallback would serve the whole
    // app unauthenticated. Verified by the "no cookie → /login" check.
    let protected = Router::new()
        .route("/ws", get(ws::handler))
        // Without the explicit limit this tops out at axum's 2 MiB default,
        // which a phone screenshot already exceeds — and the 413 arrives as a
        // silent "nothing happened" in the terminal.
        .route(
            "/blob",
            axum::routing::post(blob::upload)
                .layer(axum::extract::DefaultBodyLimit::max(blob::MAX_BYTES)),
        )
        // Whole-file transfer for the file tree: upload into a directory, and
        // download back out through the browser's own save UI.
        .route(
            "/files",
            axum::routing::post(files::upload).get(files::download),
        )
        .fallback_service(static_files)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let app = Router::new()
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/healthz", get(|| async { "ok" }))
        // Outside `require_auth` on purpose: an agent posting into the room has
        // a bearer token, not a browser session. The handler does its own check
        // and answers 401 with no detail.
        .route("/room/post", axum::routing::post(room::http::post))
        // Same reasoning: a peer server presents its own long-lived token on
        // the upgrade request, not a browser session cookie.
        .route("/room/ws", get(room::peer::hub_ws))
        // A remote client — a phone, the desktop app — with a room-only token.
        // It authenticates in its first frame, since a browser cannot set a
        // header on a WebSocket handshake.
        .route("/room/client", get(room::client::handler))
        // The room for a phone: a page that holds nothing until given a client
        // token, so it needs no login — and must not require the one that
        // reaches a shell.
        .route("/room", get(room::client::page))
        // Unauthenticated on purpose: a browser holding a stale service-worker
        // registration must be able to fetch its replacement to retire it, and
        // a 401 here would leave that worker installed forever.
        .route_service("/sw.js", ServeFile::new(web_root.join("sw.js")))
        .merge(protected)
        .layer(CompressionLayer::new())
        .layer(axum::middleware::from_fn(cache_control))
        .with_state(state.clone());

    let scheme = if tls.is_some() { "https" } else { "http" };
    log::info!("phantom-server listening on {scheme}://{addr} (web root: {})", web_root.display());

    match tls {
        Some(tls) => {
            let rustls = RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "loading TLS certificate ({} / {}) failed: {e}",
                        tls.cert.display(),
                        tls.key.display()
                    )
                });
            axum_server::bind_rustls(addr, rustls)
                .serve(app.into_make_service())
                .await
                .expect("server error");
        }
        None => {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .expect("server error");
        }
    }
}

/// Runs as nothing but a room peer.
///
/// The machine hosts agents for someone else's room; it has no use for the
/// editor, the terminal surface, or a login form, and no reason to be reachable
/// from the network at all. What it still needs is a way for its own agents to
/// post — `say` and the MCP tool both go through HTTP — so one loopback
/// listener stays, carrying that endpoint and nothing else.
///
/// Everything the peer does *outbound* — joining the room, receiving mentions,
/// serving reference fetches — rides the connection it dials to the hub, which
/// is already running by the time this is called.
async fn serve_peer_only(state: Arc<AppState>) {
    // Without a hub this mode does nothing at all: no room to join, and a
    // loopback endpoint whose posts would go nowhere. Say so rather than
    // sitting there looking healthy.
    if state.config.room.hub.is_none() {
        log::error!("`room.peer-only` is set but `room.hub` is not; nothing to join");
        std::process::exit(1);
    }

    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, state.config.addr.port()));
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        // The one endpoint. Bearer-authenticated per agent, exactly as on a
        // full server — loopback is not treated as trusted.
        .route("/room/post", axum::routing::post(room::http::post))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));
    log::info!("peer-only: agents post at http://{addr}; no web UI is served");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

/// Cache policy for the SPA. Vite fingerprints everything under `/assets/`, so
/// those are safe to cache forever; the shell (`index.html`, `sw.js`, and the
/// SPA fallback) must revalidate every load, or a browser holding a stale
/// `index.html` after a redeploy requests fingerprinted chunks that no longer
/// exist and renders a blank page.
async fn cache_control(req: Request, next: Next) -> Response {
    let immutable = req.uri().path().starts_with("/assets/");
    let mut resp = next.run(req).await;
    let value = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(value));
    resp
}

/// Two independent ways to hand out a shell by accident; both deserve a shout.
fn warn_about_exposure(config: &Config) {
    let reachable = !config.addr.ip().is_loopback();

    if config.auth == Auth::None && reachable {
        log::error!(
            "auth is disabled AND {} is reachable from the network — anyone who can \
             reach this address can open a shell as this user",
            config.addr
        );
    }

    if config.tls.is_none() && reachable {
        log::warn!(
            "serving plain HTTP on {}: the password crosses the network in the clear, \
             and the browser withholds clipboard, notification, push, and service-worker \
             APIs outside a secure context. Set `cert` and `cert-key` to fix both.",
            config.addr
        );
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    log::info!("shutting down");
}
