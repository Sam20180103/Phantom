use futures_util::future::Abortable;
pub use phantom_core::net::stream::AiStreamState;
use phantom_core::net::{self, HttpResponse, StreamRequest};
use serde::Deserialize;
use std::collections::HashMap;
use tauri::ipc::{JavaScriptChannelId, Request, Response};
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn lm_ping(base_url: String) -> Result<u16, String> {
    net::lm_ping(base_url).await
}

#[tauri::command]
pub async fn ai_http_request(
    url: String,
    method: String,
    headers: Option<HashMap<String, String>>,
    body: Option<Vec<u8>>,
    allow_private_network: Option<bool>,
) -> Result<HttpResponse, String> {
    net::ai_http_request(url, method, headers, body, allow_private_network).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    request_id: String,
    #[serde(flatten)]
    request: StreamRequest,
    on_event: JavaScriptChannelId,
}

#[tauri::command]
pub async fn ai_http_stream(
    window: WebviewWindow,
    state: State<'_, AiStreamState>,
    request: Request<'_>,
) -> Result<(), String> {
    let metadata = request
        .headers()
        .get("x-terax-ai-request")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 64 * 1024)
        .ok_or("missing or oversized AI request metadata")?;
    let mut metadata: Metadata = serde_json::from_str(metadata).map_err(|e| e.to_string())?;
    let body = super::ipc_body::bytes_of(
        request.body(),
        64 * 1024 * 1024,
        super::ipc_body::JSON_FALLBACK_MAX_BYTES,
    )?;
    metadata.request.body = if body.is_empty() {
        None
    } else {
        Some(body.into_owned())
    };
    let channel = metadata
        .on_event
        .channel_on::<_, Response>(window.as_ref().clone());
    let (lease, registration) = state.start(window.label(), &metadata.request_id)?;
    tauri::async_runtime::spawn(async move {
        let run = net::ai_http_stream_controlled(metadata.request, &lease.control, |packet| {
            channel
                .send(Response::new(packet))
                .map_err(|e| e.to_string())
        });
        let _ = Abortable::new(run, registration).await;
        drop(lease);
    });
    Ok(())
}

#[tauri::command]
pub async fn ai_http_cancel(
    window: WebviewWindow,
    state: State<'_, AiStreamState>,
    request_id: String,
) -> Result<(), String> {
    state.cancel(window.label(), &request_id);
    Ok(())
}

#[tauri::command]
pub async fn ai_http_ack(
    window: WebviewWindow,
    state: State<'_, AiStreamState>,
    request_id: String,
    bytes: u64,
) -> Result<(), String> {
    state.acknowledge(window.label(), &request_id, bytes)
}
