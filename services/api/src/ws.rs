//! WebSocket handlers: `/v1/ingest` (pings up) and `/v1/live` (vehicle states
//! down), both speaking the protobuf contract in `proto/tabaani/v1`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::StreamExt;

use crate::pb::{LiveFrame, PingBatch};
use crate::state::StoredVehicle;
use crate::AppState;

/// Cap a single batch so a malformed client cannot exhaust memory.
const MAX_BATCH_BYTES: usize = 64 * 1024;

pub async fn ingest_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ingest_loop(socket, state))
}

async fn ingest_loop(mut socket: WebSocket, state: AppState) {
    // Track per-connection sequence numbers so retries never double-count.
    let mut last_seq_end: u32 = 0;
    while let Some(Ok(msg)) = socket.next().await {
        let Message::Binary(bytes) = msg else {
            let _ = socket
                .send(Message::Text(
                    "{\"error\":\"expected binary protobuf PingBatch\"}\n".into(),
                ))
                .await;
            continue;
        };
        if bytes.len() > MAX_BATCH_BYTES {
            let _ = socket
                .send(Message::Text("{\"error\":\"batch too large\"}\n".into()))
                .await;
            continue;
        }
        match PingBatch::decode(&bytes) {
            Ok(batch) => {
                let end = batch.seq_start.saturating_add(batch.fixes.len() as u32);
                if end <= last_seq_end && !batch.fixes.is_empty() {
                    tracing::debug!(session = %batch.session_token, "duplicate batch ignored");
                    continue;
                }
                match enqueue(&state, &batch).await {
                    Ok(n) => {
                        tracing::info!(
                            session = %batch.session_token,
                            fixes = n,
                            "enqueued"
                        );
                        let _ = socket
                            .send(Message::Text(format!("{{\"accepted\":{n}}}\n").into()))
                            .await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "enqueue failed");
                        let _ = socket
                            .send(Message::Text("{\"error\":\"enqueue failed\"}\n".into()))
                            .await;
                    }
                }
                last_seq_end = end;
            }
            Err(e) => {
                tracing::debug!(error = %e, "malformed batch");
                let _ = socket
                    .send(Message::Text("{\"error\":\"malformed batch\"}\n".into()))
                    .await;
            }
        }
    }
}

/// Push a batch onto the matcher stream. Uses Redis Streams XADD so multiple
/// matcher replicas share the work through a consumer group.
///
/// Wire convention (see `Fix` in proto): the FIRST fix in a batch carries
/// absolute lat/lon/timestamp; every later fix carries deltas from its
/// predecessor. That keeps the payload small while making every batch
/// self-describing, so no out-of-band origin is needed.
async fn enqueue(state: &AppState, batch: &PingBatch) -> anyhow::Result<usize> {
    let client = redis::Client::open(state.redis_url())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let payload = batch.encode();
    let _: String = redis::cmd("XADD")
        .arg("pings:{default}")
        .arg("*")
        .arg("token")
        .arg(&batch.session_token)
        .arg("seq_start")
        .arg(batch.seq_start)
        .arg("dataset_version")
        .arg(&batch.dataset_version)
        .arg("payload")
        .arg(&payload)
        .query_async(&mut conn)
        .await?;
    Ok(payload.len())
}

/// Live feed: subscribe to route-scoped pub/sub channels and forward deltas.
pub async fn live_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| live_loop(socket, state))
}

async fn live_loop(mut socket: WebSocket, state: AppState) {
    let client = match redis::Client::open(state.redis_url()) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "live: bad redis url");
            return;
        }
    };
    let Ok(conn) = client.get_async_pubsub().await else {
        return;
    };
    let mut pubsub = conn;
    // A single catch-all channel keeps v1 simple; routes are filtered client-side.
    if let Err(e) = pubsub.subscribe("live:all").await {
        tracing::warn!(error = %e, "live: subscribe failed");
        return;
    }
    let mut stream = pubsub.into_on_message();

    // Send the current snapshot first so a fresh client is not blank.
    let frame = snapshot(&state).await.unwrap_or_default();
    let encoded: Bytes = frame.encode().into();
    if socket.send(Message::Binary(encoded)).await.is_err() {
        return;
    }

    while let Some(msg) = stream.next().await {
        let payload: Bytes = msg.get_payload_bytes().to_vec().into();
        if socket.send(Message::Binary(payload)).await.is_err() {
            return;
        }
    }
}

/// Build a `LiveFrame` from the Redis live-state hashes.
pub async fn snapshot(state: &AppState) -> anyhow::Result<LiveFrame> {
    let client = redis::Client::open(state.redis_url())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let ids: Vec<String> = redis::cmd("SMEMBERS")
        .arg("veh:ids")
        .query_async(&mut conn)
        .await?;
    let mut vehicles = Vec::with_capacity(ids.len());
    for vid in ids {
        let key = format!("veh:{vid}");
        let json: Option<String> = redis::cmd("GET").arg(&key).query_async(&mut conn).await?;
        let Some(json) = json else { continue };
        if let Ok(v) = serde_json::from_str::<StoredVehicle>(&json) {
            vehicles.push(v.into_state());
        }
    }
    Ok(LiveFrame { vehicles })
}

/// Unused-import guard: `Arc` is used by callers that wrap AppState.
#[allow(dead_code)]
fn _arc_marker(_: Arc<()>) {}
