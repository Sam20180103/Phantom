use crate::session::Outbound;
use futures_util::future::Abortable;
use phantom_core::net::{self, stream::AiStreamState, StreamRequest};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamArgs {
    request_id: String,
    #[serde(flatten)]
    request: StreamRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelArgs {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlArgs {
    request_id: String,
    bytes: u64,
}

pub async fn dispatch(
    cmd: &str,
    args: Value,
    req: u64,
    tx: &mpsc::UnboundedSender<Outbound>,
    state: &AiStreamState,
) -> Option<Result<Value, String>> {
    Some(match cmd {
        "lm_ping" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct PingArgs {
                base_url: String,
            }
            match parse::<PingArgs>(args) {
                Ok(a) => net::lm_ping(a.base_url).await.map(|s| serde_json::json!(s)),
                Err(e) => Err(e),
            }
        }
        "ai_http_ack" => parse::<ControlArgs>(args)
            .and_then(|a| state.acknowledge("socket", &a.request_id, a.bytes))
            .map(|()| Value::Null),
        "ai_http_cancel" => parse::<CancelArgs>(args).map(|a| {
            state.cancel("socket", &a.request_id);
            Value::Null
        }),
        "ai_http_stream" => match parse::<StreamArgs>(args) {
            Ok(a) => match state.start("socket", &a.request_id) {
                Ok((lease, registration)) => {
                    let run = net::ai_http_stream_controlled(a.request, &lease.control, |packet| {
                        let mut frame = Vec::with_capacity(9 + packet.len());
                        frame.push(0x04);
                        frame.extend_from_slice(&req.to_le_bytes());
                        frame.extend_from_slice(&packet);
                        tx.send(Outbound::Binary(frame))
                            .map_err(|_| "AI socket closed".into())
                    });
                    tokio::select! {
                        result = Abortable::new(run, registration) => result.unwrap_or(Ok(())).map(|()| Value::Null),
                        _ = tx.closed() => Ok(Value::Null),
                    }
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}
