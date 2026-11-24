//! `phantom-server say <text>` — the room's command-line entry point.
//!
//! This is the fallback that makes the room reachable from *any* agent: a CLI
//! that speaks MCP uses the tool, and everything else runs one command. Both end
//! up at `POST /room/post`, so there is one implementation of what posting means.
//!
//! Credentials come from the environment rather than flags, so a token never
//! lands in shell history or in a process listing:
//!
//! * `PHANTOM_ROOM_TOKEN` — the agent's token from the config's `room:` block.
//! * `PHANTOM_ROOM_URL` — server base URL; defaults to the configured bind address.

use crate::config::Config;

pub async fn run(args: &[String]) -> Result<(), String> {
    let (text, handoffs) = parse_args(args)?;
    let token = std::env::var("PHANTOM_ROOM_TOKEN")
        .map_err(|_| "PHANTOM_ROOM_TOKEN is not set".to_string())?;

    // The same config the server reads, for two things: where it listens, and
    // which certificate it presents. Best effort — an explicit URL is enough to
    // post without a readable config.
    let config = Config::load().ok();
    let base = std::env::var("PHANTOM_ROOM_URL").ok().unwrap_or_else(|| {
        match &config {
            Some(c) => {
                let scheme = if c.tls.is_some() { "https" } else { "http" };
                format!("{scheme}://{}", c.addr)
            }
            None => "http://127.0.0.1:8099".to_string(),
        }
    });

    let resp = client(config.as_ref())?
        .post(format!("{}/room/post", base.trim_end_matches('/')))
        .bearer_auth(token)
        .json(&serde_json::json!({ "text": text, "handoffs": handoffs }))
        .send()
        .await
        .map_err(|e| format!("posting to {base} failed: {}", chain(&e)))?;

    let status = resp.status();
    if !status.is_success() {
        let why = resp.text().await.unwrap_or_default();
        return Err(match why.trim() {
            "" => format!("room refused the message: {status}"),
            why => format!("room refused the message: {status}: {why}"),
        });
    }
    Ok(())
}

/// `say [--ref FILE]... TEXT...`: the text, and the files from this machine's
/// `.phantom/handoff/` to attach. Only a word that is exactly `--ref` (or
/// `--ref=FILE`) is the flag, so text that merely mentions it stays text.
fn parse_args(args: &[String]) -> Result<(String, Vec<String>), String> {
    let mut handoffs = Vec::new();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--ref" {
            handoffs.push(args.next().ok_or("--ref needs a file name")?.clone());
        } else if let Some(file) = arg.strip_prefix("--ref=") {
            handoffs.push(file.to_string());
        } else {
            words.push(arg.as_str());
        }
    }
    Ok((words.join(" "), handoffs))
}

/// Trusts the server's own certificate, in addition to the system roots.
///
/// The recommended setup (`scripts/gen-phantom-cert.sh`) is a self-signed cert
/// on a private mesh, which no system root chains to — so `say` would fail
/// against exactly the deployment the README describes. Adding *that* file as a
/// trust anchor is not the same as skipping verification: an impostor still has
/// to present this certificate, and anything else is still rejected.
fn client(config: Option<&Config>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();

    // Both, because they cover different deployments and neither implies the
    // other. A self-signed host certificate validates as its own anchor, so it
    // is what a single machine needs; once a mesh CA signs that certificate the
    // leaf is no longer self-signed and only the *issuer* validates the chain.
    // Adding just one of them silently broke `say` the day a CA was introduced.
    let anchors = config.into_iter().flat_map(|c| {
        c.room
            .ca
            .clone()
            .into_iter()
            .chain(c.tls.as_ref().map(|t| t.cert.clone()))
    });

    for path in anchors {
        // A missing or unreadable file is not fatal: the URL may point at a
        // host whose chain the system roots already cover.
        match std::fs::read(&path) {
            Ok(pem) => match reqwest::Certificate::from_pem(&pem) {
                Ok(cert) => builder = builder.add_root_certificate(cert),
                Err(e) => log::debug!("ignoring {}: {e}", path.display()),
            },
            Err(e) => log::debug!("ignoring {}: {e}", path.display()),
        }
    }

    builder
        .build()
        .map_err(|e| format!("building the HTTP client failed: {e}"))
}

/// Flattens an error's source chain.
///
/// `reqwest`'s own `Display` is just "error sending request", which hides
/// whether the cause was a refused connection or a rejected certificate — the
/// two failures a first run actually hits.
fn chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        parts.push(s.to_string());
        source = s.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn refs_are_pulled_out_of_the_text() {
        let (text, refs) =
            parse_args(&args(&["@alpha/claude", "--ref", "api.md", "done", "--ref=notes.md"])).unwrap();
        assert_eq!(text, "@alpha/claude done");
        assert_eq!(refs, vec!["api.md", "notes.md"]);
    }

    #[test]
    fn text_that_mentions_the_flag_is_left_alone() {
        let (text, refs) = parse_args(&args(&["use --ref to attach"])).unwrap();
        assert_eq!(text, "use --ref to attach");
        assert!(refs.is_empty());
        assert!(parse_args(&args(&["hi", "--ref"])).is_err());
    }
}
