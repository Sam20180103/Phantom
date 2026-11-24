use crate::net::{ai_http_request, ai_http_stream, lm_ping, tests::runtime, AiStreamEvent};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    task: Option<JoinHandle<()>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let task = std::thread::spawn({
            let stop = stop.clone();
            let connections = connections.clone();
            let paths = paths.clone();
            move || {
                let mut workers = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("accept: {error}"),
                    };
                    let id = connections.fetch_add(1, Ordering::Relaxed) + 1;
                    let stop = stop.clone();
                    let paths = paths.clone();
                    socket
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    workers.push(std::thread::spawn(move || {
                        let mut reader = BufReader::new(socket.try_clone().unwrap());
                        while !stop.load(Ordering::Relaxed) {
                            let mut first = String::new();
                            match reader.read_line(&mut first) {
                                Ok(0) => break,
                                Ok(_) => {},
                                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                                Err(_) => break,
                            }
                            let mut authorization = None;
                            loop {
                                let mut line = String::new();
                                let read = reader.read_line(&mut line).unwrap();
                                if line == "\r\n" || read == 0 { break; }
                                if let Some((name, value)) = line.split_once(':') {
                                    if name.eq_ignore_ascii_case("authorization") { authorization = Some(value.trim().to_string()); }
                                }
                            }
                            let path = first.split_whitespace().nth(1).unwrap().to_string();
                            paths.lock().unwrap().push(path.clone());
                            let response = if path == "/models" {
                                "HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n".into()
                            } else {
                                let body = format!("connection={id};authorization={}", authorization.as_deref().unwrap_or("none"));
                                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                            };
                            if socket.write_all(response.as_bytes()).is_err() { break; }
                        }
                    }));
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            }
        });
        Self {
            port,
            stop,
            connections,
            paths,
            task: Some(task),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.task.take().unwrap().join().unwrap();
    }
}

#[test]
fn requests_and_streams_reuse_the_same_live_http_connection() {
    let server = Server::start();
    runtime().block_on(async {
        for _ in 0..2 {
            let response =
                ai_http_request(server.url("/request"), "GET".into(), None, None, Some(true))
                    .await
                    .unwrap();
            assert_eq!(response.body, b"connection=1;authorization=none");
            tokio::task::yield_now().await;
        }
        let mut bytes = Vec::new();
        ai_http_stream(
            server.url("/stream"),
            "GET".into(),
            None,
            None,
            Some(true),
            |event| {
                if let AiStreamEvent::Chunk { bytes: chunk } = event {
                    bytes.extend(chunk);
                }
                true
            },
        )
        .await
        .unwrap();
        assert_eq!(bytes, b"connection=1;authorization=none");
    });
    assert_eq!(server.connections(), 1);
}

#[test]
fn ping_has_a_separate_pool_and_never_inherits_provider_redirect_policy() {
    let server = Server::start();
    runtime().block_on(async {
        assert_eq!(lm_ping(server.url("")).await.unwrap(), 302);
        tokio::task::yield_now().await;
        assert_eq!(lm_ping(server.url("")).await.unwrap(), 302);
        tokio::task::yield_now().await;
        assert_eq!(server.connections(), 1);
        assert_eq!(*server.paths.lock().unwrap(), ["/models", "/models"]);
        let response = ai_http_request(server.url("/models"), "GET".into(), None, None, Some(true))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"connection=2;authorization=none");
    });
    assert_eq!(server.connections(), 2);
    assert_eq!(
        *server.paths.lock().unwrap(),
        ["/models", "/models", "/models", "/redirected"]
    );
}

#[test]
fn credentials_are_per_request_even_when_the_connection_is_reused() {
    let server = Server::start();
    runtime().block_on(async {
        for credential in [Some("Bearer first"), None, Some("Bearer second")] {
            let headers =
                credential.map(|value| HashMap::from([("Authorization".into(), value.into())]));
            let response = ai_http_request(
                server.url("/request"),
                "GET".into(),
                headers,
                None,
                Some(true),
            )
            .await
            .unwrap();
            assert_eq!(
                String::from_utf8(response.body).unwrap(),
                format!(
                    "connection=1;authorization={}",
                    credential.unwrap_or("none")
                )
            );
            tokio::task::yield_now().await;
        }
    });
    assert_eq!(server.connections(), 1);
}
