use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    task: Option<JoinHandle<()>>,
}

impl Server {
    fn start(respond: impl Fn(&str) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task = std::thread::spawn({
            let stop = stop.clone();
            let requests = requests.clone();
            move || {
                while !stop.load(Ordering::Relaxed) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("accept: {error}"),
                    };
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(&socket);
                    let mut first = String::new();
                    if reader.read_line(&mut first).is_err() {
                        continue;
                    }
                    let mut size = first.len();
                    loop {
                        let mut line = String::new();
                        let Ok(read) = reader.read_line(&mut line) else {
                            break;
                        };
                        size += read;
                        if line == "\r\n" || read == 0 || size > 8192 {
                            break;
                        }
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                    requests.lock().unwrap().push(path.clone());
                    let _ = socket.write_all(respond(&path).as_bytes());
                }
            }
        });
        Self {
            port,
            stop,
            requests,
            task: Some(task),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    fn paths(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.take().unwrap().join().unwrap();
    }
}

fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn redirect(location: &str) -> String {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

#[test]
fn private_access_requires_opt_in_and_local_models_still_work() {
    let server = Server::start(|_| ok("local model"));
    tests::runtime().block_on(async {
        assert!(
            ai_http_request(server.url("/models"), "GET".into(), None, None, None)
                .await
                .unwrap_err()
                .contains("explicit opt-in")
        );
        assert!(server.paths().is_empty());
        let response = ai_http_request(server.url("/models"), "GET".into(), None, None, Some(true))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"local model");
        assert_eq!(lm_ping(server.url("")).await.unwrap(), 200);
    });
    assert_eq!(server.paths(), ["/models", "/models"]);
}

#[test]
fn safe_relative_redirects_follow_but_new_hostnames_and_metadata_do_not() {
    let server = Server::start(|path| match path {
        "/same" => redirect("/ok"),
        "/new-host" => redirect("http://unclassified.invalid/"),
        "/metadata" => redirect("http://[::ffff:169.254.169.254]/latest"),
        _ => ok("safe"),
    });
    tests::runtime().block_on(async {
        let response = ai_http_request(server.url("/same"), "GET".into(), None, None, Some(true))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"safe");
        for path in ["/new-host", "/metadata"] {
            let response = ai_http_request(server.url(path), "GET".into(), None, None, Some(true))
                .await
                .unwrap();
            assert_eq!(response.status, 302);
        }
    });
    assert_eq!(server.paths(), ["/same", "/ok", "/new-host", "/metadata"]);
}

#[test]
fn pinned_hostname_redirects_reuse_only_the_validated_resolver_override() {
    let server = Server::start(|path| {
        if path == "/first" {
            redirect("/second")
        } else {
            ok("pinned")
        }
    });
    tests::runtime().block_on(async {
        let client = client_for(
            Profile::Stream,
            "pinned.invalid",
            &["127.0.0.1".parse().unwrap()],
            true,
        )
        .unwrap();
        let response = client
            .get(format!("http://pinned.invalid:{}/first", server.port))
            .send()
            .await
            .unwrap();
        assert_eq!(response.bytes().await.unwrap(), "pinned");
    });
    assert_eq!(server.paths(), ["/first", "/second"]);
}

#[test]
fn streaming_keeps_local_opt_in_and_redirect_security_without_changing_events() {
    let server = Server::start(|path| match path {
        "/same" => redirect("/sse"),
        "/blocked" => redirect("http://unclassified.invalid/"),
        _ => ok("data: local\n\n"),
    });
    tests::runtime().block_on(async {
        let mut refused = Vec::new();
        assert!(ai_http_stream(server.url("/sse"), "GET".into(), None, None, None, |event| {
            refused.push(event);
            true
        }).await.unwrap_err().contains("explicit opt-in"));
        assert!(matches!(&refused[..], [AiStreamEvent::Error { .. }]));
        assert!(server.paths().is_empty());
        for (path, status, expected) in [("/same", 200, b"data: local\n\n".as_slice()), ("/blocked", 302, b"".as_slice())] {
            let mut events = Vec::new();
            ai_http_stream(server.url(path), "GET".into(), None, None, Some(true), |event| {
                events.push(event);
                true
            }).await.unwrap();
            assert!(matches!(events.first(), Some(AiStreamEvent::Headers { status: actual, .. }) if *actual == status));
            assert!(matches!(events.last(), Some(AiStreamEvent::End)));
            let body: Vec<_> = events.iter().flat_map(|event| match event {
                AiStreamEvent::Chunk { bytes } => bytes.as_slice(),
                _ => &[],
            }).copied().collect();
            assert_eq!(body, expected);
        }
    });
    assert_eq!(server.paths(), ["/same", "/sse", "/blocked"]);
}

#[test]
fn ambient_proxies_cannot_bypass_the_pinned_resolver() {
    const CHILD_URL: &str = "PHANTOM_NET_TEST_CHILD_URL";
    if let Ok(url) = std::env::var(CHILD_URL) {
        tests::runtime().block_on(async {
            let response = ai_http_request(url.clone(), "GET".into(), None, None, Some(true))
                .await
                .unwrap();
            assert_eq!(response.body, b"direct");
            assert_eq!(lm_ping(url.clone()).await.unwrap(), 200);
            let mut body = Vec::new();
            ai_http_stream(url, "GET".into(), None, None, Some(true), |event| {
                if let AiStreamEvent::Chunk { bytes } = event {
                    body.extend(bytes);
                }
                true
            })
            .await
            .unwrap();
            assert_eq!(body, b"direct");
        });
        return;
    }
    let server = Server::start(|_| ok("direct"));
    let proxy = Server::start(|_| ok("unexpected proxy"));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "net::integration_tests::ambient_proxies_cannot_bypass_the_pinned_resolver",
            "--nocapture",
        ])
        .env(CHILD_URL, server.url(""))
        .env("HTTP_PROXY", proxy.url(""))
        .env("http_proxy", proxy.url(""))
        .env("HTTPS_PROXY", proxy.url(""))
        .env("https_proxy", proxy.url(""))
        .env("ALL_PROXY", proxy.url(""))
        .env("all_proxy", proxy.url(""))
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(proxy.paths().is_empty());
    assert_eq!(server.paths(), ["/", "/models", "/"]);
}
