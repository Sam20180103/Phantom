//! Password login and session checking.
//!
//! Deliberately small: a shared password (optionally paired with a username), a
//! login form, and an opaque session token in an HttpOnly cookie. That is the
//! same threat model code-server uses, and it is the right one here — a single
//! operator reaching their own machine over a private mesh. The credentials are
//! read from the config file (see `config::Auth`); this module only compares
//! what the form sent against them.
//!
//! Sessions live in memory, so restarting the server logs every device out.
//! Acceptable for now; persisting them would mean persisting a secret, which
//! needs more care than it buys.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Form, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::config::Auth;
use crate::AppState;

pub const COOKIE_NAME: &str = "phantom-session";

#[derive(Default)]
pub struct Sessions {
    tokens: Mutex<HashSet<String>>,
}

impl Sessions {
    fn issue(&self) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("system RNG unavailable");
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.tokens.lock().expect("sessions poisoned").insert(token.clone());
        token
    }

    fn is_valid(&self, token: &str) -> bool {
        self.tokens.lock().expect("sessions poisoned").contains(token)
    }
}

/// Compares in constant time so a network observer can't recover a secret one
/// character at a time from response timing.
fn secret_ct_eq(expected: &str, given: &str) -> bool {
    expected.as_bytes().ct_eq(given.as_bytes()).into()
}

/// Whole-credential check for the login form.
///
/// `expected_user == None` means this deployment authenticates on the password
/// alone (a reused code-server config), so the form's username field is ignored.
///
/// Both comparisons run regardless of the first result: short-circuiting on the
/// username would let response timing reveal whether the name was right before
/// the password was ever considered, turning one secret into two easier ones.
fn credentials_match(
    expected_user: Option<&str>,
    expected_pass: &str,
    given_user: &str,
    given_pass: &str,
) -> bool {
    let user_ok = match expected_user {
        Some(expected) => secret_ct_eq(expected, given_user),
        None => true,
    };
    let pass_ok = secret_ct_eq(expected_pass, given_pass);
    user_ok & pass_ok
}

/// Finds a cookie across **every** `Cookie` header, not just the first.
///
/// HTTP/2 permits splitting cookies into several header fields (RFC 7540
/// §8.1.2.5) and browsers do exactly that once a site has more than a couple.
/// Reading only `headers.get(COOKIE)` therefore finds the session cookie or not
/// depending on which group the browser happened to put it in — and curl, which
/// always sends one combined header, never reproduces it.
fn cookie_value<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

pub fn is_authenticated(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    match &state.config.auth {
        Auth::None => true,
        Auth::Password { .. } => cookie_value(headers, COOKIE_NAME)
            .map(|t| state.sessions.is_valid(t))
            .unwrap_or(false),
    }
}

/// Gate for everything except `/login` and `/healthz`.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if is_authenticated(&state, request.headers()) {
        return next.run(request).await;
    }

    // Diagnostic: a rejected request is either missing the cookie entirely
    // (browser refused to store it) or presenting one we don't know (server
    // restarted, wiping in-memory sessions). Those need different fixes, and
    // the header tells us which without guessing.
    let cookie_header = request.headers().get(header::COOKIE).is_some();
    let has_session = cookie_value(request.headers(), COOKIE_NAME).is_some();
    log::info!(
        "auth rejected {} {} (Cookie header: {}, {COOKIE_NAME}: {})",
        request.method(),
        request.uri().path(),
        if cookie_header { "present" } else { "absent" },
        if has_session { "present but unknown" } else { "absent" },
    );

    // A browser navigating should land on the form; anything else (fetch, the
    // WebSocket) gets a status it can act on rather than a page of HTML.
    let wants_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("text/html"))
        .unwrap_or(false);

    if wants_html {
        Redirect::to("/login").into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

#[derive(Deserialize)]
pub struct LoginForm {
    /// Always sent by the form; only checked when the config sets `username`.
    #[serde(default)]
    username: String,
    password: String,
}

pub async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    let Auth::Password { username, password } = &state.config.auth else {
        return Redirect::to("/").into_response();
    };

    if !credentials_match(
        username.as_deref(),
        password,
        form.username.trim(),
        &form.password,
    ) {
        log::warn!("failed login attempt");
        return (StatusCode::UNAUTHORIZED, Html(LOGIN_HTML_FAILED)).into_response();
    }

    let token = state.sessions.issue();
    // `Secure` is set only under TLS: with it on a plain-HTTP origin the
    // browser silently drops the cookie and login appears to do nothing.
    let secure = if state.config.tls.is_some() { "; Secure" } else { "" };
    let cookie = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure}"
    );

    log::info!("login succeeded");
    (
        StatusCode::SEE_OTHER,
        [(header::SET_COOKIE, cookie), (header::LOCATION, "/".into())],
    )
        .into_response()
}

const LOGIN_HTML: &str = include_str!("login.html");
const LOGIN_HTML_FAILED: &str = include_str!("login_failed.html");

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn secret_comparison_is_exact() {
        assert!(secret_ct_eq("hunter2", "hunter2"));
        assert!(!secret_ct_eq("hunter2", "hunter"));
        assert!(!secret_ct_eq("hunter2", "hunter3"));
        assert!(!secret_ct_eq("hunter2", ""));
        // Length differences must not match either. ct_eq handles this, but
        // it's the case a naive prefix comparison would get wrong.
        assert!(!secret_ct_eq("", "x"));
    }

    #[test]
    fn credentials_need_both_fields_when_username_is_set() {
        assert!(credentials_match(Some("admin"), "pw", "admin", "pw"));
        assert!(!credentials_match(Some("admin"), "pw", "admin", "wrong"));
        assert!(!credentials_match(Some("admin"), "pw", "wrong", "pw"));
        assert!(!credentials_match(Some("admin"), "pw", "", ""));
    }

    #[test]
    fn password_only_config_ignores_the_username_field() {
        assert!(credentials_match(None, "pw", "", "pw"));
        assert!(credentials_match(None, "pw", "anything", "pw"));
        assert!(!credentials_match(None, "pw", "", "nope"));
    }

    #[test]
    fn parses_cookie_out_of_a_crowded_header() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            "other=1; phantom-session=abc123; theme=dark".parse().unwrap(),
        );
        assert_eq!(cookie_value(&h, COOKIE_NAME), Some("abc123"));
    }

    #[test]
    fn missing_cookie_is_not_an_error() {
        assert_eq!(cookie_value(&HeaderMap::new(), COOKIE_NAME), None);
    }

    // Regression: browsers split cookies across several `Cookie` headers over
    // HTTP/2, so the session cookie can land in any of them. Reading only the
    // first made login appear to succeed and then bounce straight back to the
    // form. curl never reproduced it — it always sends one combined header.
    #[test]
    fn finds_cookie_split_across_multiple_headers() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, "code-server-session=abc".parse().unwrap());
        h.append(header::COOKIE, "analytics=xyz".parse().unwrap());
        h.append(header::COOKIE, "phantom-session=wanted".parse().unwrap());
        assert_eq!(cookie_value(&h, COOKIE_NAME), Some("wanted"));
    }

    #[test]
    fn finds_cookie_in_a_later_header_group() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, "a=1; b=2".parse().unwrap());
        h.append(header::COOKIE, "phantom-session=tok; c=3".parse().unwrap());
        assert_eq!(cookie_value(&h, COOKIE_NAME), Some("tok"));
    }

    #[test]
    fn issued_tokens_validate_and_are_unique() {
        let s = Sessions::default();
        let a = s.issue();
        let b = s.issue();
        assert_ne!(a, b);
        assert!(s.is_valid(&a) && s.is_valid(&b));
        assert!(!s.is_valid("nope"));
    }
}
