use super::stream::{self, StreamControl};
use super::*;
use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRequest {
    pub url: String,
    pub method: String,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<Vec<u8>>,
    pub allow_private_network: Option<bool>,
}

pub async fn ai_http_stream_controlled(
    request: StreamRequest,
    control: &StreamControl,
    mut send: impl FnMut(Vec<u8>) -> Result<(), String>,
) -> Result<(), String> {
    let run = async {
        if request
            .body
            .as_ref()
            .is_some_and(|body| body.len() > 64 * 1024 * 1024)
        {
            return Err("AI request body exceeds 64 MiB".into());
        }
        let allow_private = request.allow_private_network.unwrap_or(false);
        let parsed = validate_url(&request.url, allow_private)?;
        let host = parsed.host_str().ok_or("missing host")?.to_string();
        let safe_ips = classify_and_collect_safe_ips(&host, allow_private).await?;
        let client = client_for(Profile::Stream, &host, &safe_ips, allow_private)?;
        let req = build_request(
            &client,
            &request.method,
            parsed,
            request.headers,
            request.body,
        )?;
        let response = req.send().await.map_err(|e| e.to_string())?;
        let metadata = serde_json::to_vec(&serde_json::json!({
            "status": response.status().as_u16(),
            "headers": header_map_to_strings(response.headers()),
        }))
        .map_err(|e| e.to_string())?;
        let mut packet = vec![0];
        packet.extend_from_slice(&metadata);
        send(packet)?;
        pump(response.bytes_stream(), control, &mut send).await?;
        send(vec![2])
    }
    .await;
    if let Err(error) = &run {
        let mut packet = vec![3];
        packet.extend_from_slice(error.as_bytes());
        let _ = send(packet);
    }
    run
}

/// Moves a response body to the webview under byte-window flow control.
/// Network chunks that are already available are coalesced into one packet, and
/// while the window is full the next packet keeps filling from the network, so
/// a slow consumer sees fewer, larger messages rather than a backlog of tiny
/// ones.
async fn pump<S, E>(
    mut chunks: S,
    control: &stream::StreamControl,
    mut send: impl FnMut(Vec<u8>) -> Result<(), String>,
) -> Result<(), String>
where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    use stream::{fill_packet, new_packet, payload_len, CHUNK_BYTES};

    let mut packet = new_packet(1, 0);
    let mut carry = Bytes::new();
    // A network failure still delivers whatever arrived before it.
    let mut ended = false;
    let mut failure: Option<String> = None;
    loop {
        fill_packet(&mut packet, &mut carry);
        while payload_len(&packet) < CHUNK_BYTES && carry.is_empty() && !ended {
            match chunks.next().now_or_never() {
                Some(Some(Ok(next))) => {
                    carry = next;
                    fill_packet(&mut packet, &mut carry);
                }
                Some(Some(Err(e))) => {
                    failure = Some(e.to_string());
                    ended = true;
                }
                Some(None) => ended = true,
                None => break,
            }
        }
        if payload_len(&packet) == 0 {
            if ended {
                return failure.map_or(Ok(()), Err);
            }
            match chunks.next().await {
                Some(Ok(next)) => carry = next,
                Some(Err(e)) => return Err(e.to_string()),
                None => return Ok(()),
            }
            continue;
        }
        while !control.try_reserve(payload_len(&packet))? {
            let can_absorb = payload_len(&packet) < CHUNK_BYTES && carry.is_empty() && !ended;
            if can_absorb {
                tokio::select! {
                    changed = control.credit_changed() => changed?,
                    next = chunks.next() => match next {
                        Some(Ok(next)) => {
                            carry = next;
                            fill_packet(&mut packet, &mut carry);
                        }
                        Some(Err(e)) => {
                            failure = Some(e.to_string());
                            ended = true;
                        }
                        None => ended = true,
                    },
                }
            } else {
                control.credit_changed().await?;
            }
        }
        send(std::mem::replace(&mut packet, new_packet(1, 0)))?;
        if ended && carry.is_empty() {
            return failure.map_or(Ok(()), Err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod flow {
        use super::*;
        use crate::net::stream::{AiStreamState, CHUNK_BYTES};
        use std::sync::mpsc;

        type Chunks = std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, String>>>>;

        fn ready(parts: Vec<Result<Bytes, String>>) -> Chunks {
            Box::pin(futures_util::stream::iter(parts))
        }

        fn run_pump(
            chunks: Chunks,
        ) -> (
            tokio::task::JoinHandle<Result<(), String>>,
            mpsc::Receiver<Vec<u8>>,
            AiStreamState,
        ) {
            let state = AiStreamState::default();
            let (lease, _) = state.start("w", "r").unwrap();
            let (tx, rx) = mpsc::channel();
            let task = tokio::task::spawn_local(async move {
                let result = pump(chunks, &lease.control.clone(), move |packet| {
                    tx.send(packet).map_err(|e| e.to_string())
                })
                .await;
                drop(lease);
                result
            });
            (task, rx, state)
        }

        async fn local<F: std::future::Future>(future: F) -> F::Output {
            tokio::task::LocalSet::new().run_until(future).await
        }

        #[tokio::test]
        async fn many_small_ready_chunks_become_one_packet() {
            local(async {
                let parts = (0..50)
                    .map(|_| Ok(Bytes::from(vec![b'x'; 100])))
                    .collect::<Vec<_>>();
                let (task, rx, _state) = run_pump(ready(parts));
                task.await.unwrap().unwrap();
                let packets: Vec<_> = rx.try_iter().collect();
                assert_eq!(packets.len(), 1);
                assert_eq!(packets[0][0], 1);
                assert_eq!(packets[0].len(), 1 + 5000);
            })
            .await;
        }

        #[tokio::test]
        async fn bodies_larger_than_the_window_flow_as_acknowledgments_arrive() {
            local(async {
                let total = 300 * 1024;
                let (task, rx, state) = run_pump(ready(vec![Ok(Bytes::from(vec![9u8; total]))]));
                let mut received = 0usize;
                let mut largest = 0usize;
                while received < total {
                    tokio::task::yield_now().await;
                    while let Ok(packet) = rx.try_recv() {
                        largest = largest.max(packet.len() - 1);
                        received += packet.len() - 1;
                        state.acknowledge("w", "r", received as u64).unwrap();
                    }
                }
                task.await.unwrap().unwrap();
                assert_eq!(received, total);
                assert_eq!(largest, CHUNK_BYTES);
            })
            .await;
        }

        #[tokio::test]
        async fn a_full_window_stalls_the_pump_until_the_consumer_acknowledges() {
            local(async {
                let (task, rx, state) =
                    run_pump(ready(vec![Ok(Bytes::from(vec![1u8; 192 * 1024]))]));
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                let first: Vec<_> = rx.try_iter().collect();
                assert_eq!(first.len(), 2, "only two 64 KiB packets fit in the window");
                assert!(!task.is_finished());
                state.acknowledge("w", "r", (64 * 1024) as u64).unwrap();
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(rx.try_iter().count(), 1);
                task.await.unwrap().unwrap();
            })
            .await;
        }

        #[tokio::test]
        async fn network_data_arriving_while_blocked_is_coalesced_into_the_next_packet() {
            local(async {
                let (net_tx, net_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
                let chunks: Chunks =
                    Box::pin(futures_util::stream::unfold(net_rx, |mut rx| async move {
                        rx.recv().await.map(|b| (Ok::<_, String>(b), rx))
                    }));
                let (task, rx, state) = run_pump(chunks);
                net_tx.send(Bytes::from(vec![0u8; 128 * 1024])).unwrap();
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(rx.try_iter().count(), 2);
                for _ in 0..40 {
                    net_tx.send(Bytes::from_static(b"tiny")).unwrap();
                    tokio::task::yield_now().await;
                }
                assert_eq!(rx.try_iter().count(), 0, "window still full");
                state.acknowledge("w", "r", (64 * 1024) as u64).unwrap();
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                let next: Vec<_> = rx.try_iter().collect();
                assert_eq!(next.len(), 1);
                assert_eq!(next[0].len(), 1 + 160, "forty tiny chunks, one packet");
                drop(net_tx);
                state.acknowledge("w", "r", (128 * 1024) as u64).unwrap();
                task.await.unwrap().unwrap();
            })
            .await;
        }

        #[tokio::test]
        async fn a_network_error_still_delivers_what_arrived_before_it() {
            local(async {
                let (task, rx, _state) = run_pump(ready(vec![
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                    Err("connection reset".into()),
                ]));
                assert_eq!(task.await.unwrap(), Err("connection reset".to_string()));
                let packets: Vec<_> = rx.try_iter().collect();
                assert_eq!(packets.len(), 1);
                assert_eq!(&packets[0][1..], b"data: [DONE]\n\n");
            })
            .await;
        }
    }
}
