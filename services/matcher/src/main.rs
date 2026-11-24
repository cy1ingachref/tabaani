//! `tabaani` matcher worker binary.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::pb::PingBatch;
use api::state::StoredVehicle;
use matcher::{absolute_fixes, decide, Network, SessionState};
use redis::aio::MultiplexedConnection;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--selftest") {
        return selftest().await;
    }
    init_tracing();

    let redis_url = require_env("REDIS_URL")?;
    let group = std::env::var("MATCHER_CONSUMER_GROUP").unwrap_or_else(|_| "matcher".into());
    let consumer = std::env::var("MATCHER_CONSUMER").unwrap_or_else(|_| "matcher-1".into());
    let stream = std::env::var("MATCHER_STREAM").unwrap_or_else(|_| "pings:{default}".into());
    let gtfs_dir = std::env::var("GTFS_DIR").unwrap_or_else(|_| "data/gtfs/build/0.1.0-dev".into());

    let network = Network::from_gtfs(std::path::Path::new(&gtfs_dir))?;
    tracing::info!(
        routes = ?network.routes(),
        trips = network.trips.values().map(Vec::len).sum::<usize>(),
        "network loaded"
    );

    let client = redis::Client::open(redis_url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;

    // Create the consumer group once, anchored at "0" so it sees every entry added
    // after the group exists — even if the stream was deleted and recreated.
    // MKSTREAM creates the stream if needed; BUSYGROUP means a replica did it.
    let created: Result<(), _> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&stream)
        .arg(&group)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut conn)
        .await;
    if let Err(e) = created {
        // Any error other than BUSYGROUP is fatal: swallowing it produces a
        // consumer that looks alive but never receives messages.
        let msg = e.to_string();
        if !msg.contains("BUSYGROUP") {
            tracing::error!(error = %msg, "XGROUP CREATE failed");
            return Err(e.into());
        }
        tracing::info!(%group, "consumer group already exists");
    } else {
        tracing::info!(%group, "consumer group created at id 0");
    }

    tracing::info!(%stream, %group, %consumer, "matcher consuming");
    loop {
        // Re-assert the consumer group every iteration. Redis drops a group when
        // its stream key is deleted, and clients legitimately reset the stream
        // (the e2e tool does exactly that). Creating at 0 means entries added
        // after the reset are still delivered.
        if let Err(e) = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&stream)
            .arg(&group)
            .arg("0")
            .arg("MKSTREAM")
            .query_async::<()>(&mut conn)
            .await
        {
            if !e.to_string().contains("BUSYGROUP") {
                tracing::warn!(error = %e, "XGROUP ensure failed this tick");
            }
        }
        // redis-rs maps the nested XREADGROUP reply to
        // Vec<(stream_name, Vec<entry_id, Vec<field, value>>)>, which does not
        // line up with the real wire shape. Read the raw Value instead and walk
        // it explicitly so we cannot silently mis-decode.
        let raw: redis::Value = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(&group)
            .arg(&consumer)
            .arg("COUNT")
            .arg(64)
            .arg("BLOCK")
            .arg(500)
            .arg("STREAMS")
            .arg(&stream)
            .arg(">")
            .query_async(&mut conn)
            .await
            .unwrap_or(redis::Value::Nil);
        for (id, pairs) in parse_xread(&raw) {
            match handle_entry(&mut conn, &network, &id, &pairs).await {
                Ok(()) => {}
                Err(e) => tracing::warn!(error = %e, entry = %id, "entry failed"),
            }
            let _: () = redis::cmd("XACK")
                .arg(&stream)
                .arg(&group)
                .arg(id)
                .query_async(&mut conn)
                .await
                .unwrap_or(());
        }
    }
}

fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

async fn handle_entry(
    conn: &mut MultiplexedConnection,
    network: &Network,
    entry_id: &str,
    fields: &[(String, String)],
) -> anyhow::Result<()> {
    let Some(token) = field(fields, "token") else {
        anyhow::bail!("missing token");
    };
    // The payload is a length-delimited protobuf `PingBatch`: arbitrary bytes.
    // redis-rs decodes stream field values as lossy UTF-8 Strings, which would
    // corrupt any byte >= 0x80, so fetch the raw bytes straight from Redis.
    let stream = std::env::var("MATCHER_STREAM").unwrap_or_else(|_| "pings:{default}".into());
    let rows: Vec<redis::Value> = redis::cmd("XRANGE")
        .arg(&stream)
        .arg(entry_id)
        .arg(entry_id)
        .query_async(conn)
        .await?;
    let batch = decode_payload_from_rows(&rows)?;
    if batch.fixes.is_empty() {
        return Ok(());
    }

    // Load or create session state.
    let sess_key = format!("sess:{token}");
    let json: Option<String> = redis::cmd("GET").arg(&sess_key).query_async(conn).await?;
    let mut state: SessionState = json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_else(|| SessionState {
            token: token.to_string(),
            ..Default::default()
        });

    // Project this batch's fixes onto each route and keep them as observations
    // on the route that fits best. For v1 we only have one route, so project
    // onto it and accumulate.
    let fixes = absolute_fixes(&batch);
    let Some(route_id) = network.routes().into_iter().next() else {
        return Ok(());
    };
    let Some(shape) = network.shapes.get(&route_id) else {
        return Ok(());
    };
    let obs = matcher::project_fixes(shape, &fixes);
    for o in &obs {
        // Keep the observation window bounded so memory cannot grow unbounded.
        state.obs.push((o.t_s, o.s_m));
    }
    const MAX_OBS: usize = 1200;
    if state.obs.len() > MAX_OBS {
        let drop = state.obs.len() - MAX_OBS;
        state.obs.drain(0..drop);
    }
    state.last_seen_s = obs.last().map(|o| o.t_s).unwrap_or(state.last_seen_s);

    // Decide and publish when we have an identified trip.
    if let Some(rd) = decide(network, &state) {
        match rd.decision {
            transit_core::rail::RailDecision::Identified {
                ref trip_id,
                ref delay_s,
                ..
            } => {
                state.trip_id = Some(trip_id.clone());
                state.delay_s = *delay_s;
                let s_last = obs.last().map(|o| o.s_m).unwrap_or(0.0);
                let v = vehicle_for(&rd.route_id, trip_id, s_last, *delay_s);
                matcher::publish(conn, &v).await?;
                tracing::info!(trip = trip_id, delay = delay_s, "published live vehicle");
            }
            transit_core::rail::RailDecision::Ambiguous { ref candidates } => {
                tracing::debug!(?candidates, "ambiguous; not published");
            }
            transit_core::rail::RailDecision::Insufficient { ref reason } => {
                tracing::debug!(reason, "insufficient evidence");
            }
        }
    }

    let json = serde_json::to_string(&state)?;
    let _: () = redis::pipe()
        .set(&sess_key, json)
        .expire(&sess_key, 1800)
        .query_async(conn)
        .await?;
    Ok(())
}

/// Build a live vehicle from a decided trip. Confidence is ESTIMATED until we
/// have >=2 contributors on the same trip (fusion arrives with Phase 3).
fn vehicle_for(route_id: &str, trip_id: &str, s_m: f64, delay_s: f64) -> StoredVehicle {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    // The session token deliberately does not reach the public feed (§11.1);
    // identity is the route+trip pair.
    StoredVehicle {
        vehicle_id: format!("{route_id}:{trip_id}"),
        route_id: route_id.into(),
        direction: 0,
        s_m: s_m as f32,
        v_mps: 0.0,
        ts_ms: now_ms,
        confidence: api::pb::Confidence::Estimated,
        n_riders: 1,
        trip_id: trip_id.into(),
        delay_s: delay_s as f32,
        vehicle_type: "train_coach".into(),
        n_cars: 0,
    }
}

/// Walk a raw XREADGROUP reply into `(entry_id, [(field, value), ...])`.
///
/// Expected shape (Redis 7):
/// ```text
/// [ [ stream_name, [ [ id, [f1, v1, f2, v2, ...] ], ... ] ] ]
/// ```
fn parse_xread(v: &redis::Value) -> Vec<(String, Vec<(String, String)>)> {
    let redis::Value::Array(streams) = v else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stream in streams {
        let redis::Value::Array(parts) = stream else {
            continue;
        };
        let Some(redis::Value::Array(entries)) = parts.get(1) else {
            continue;
        };
        for entry in entries {
            let redis::Value::Array(pair) = entry else {
                continue;
            };
            let Some(redis::Value::BulkString(id_bytes)) = pair.first() else {
                continue;
            };
            let id = String::from_utf8_lossy(id_bytes).into_owned();
            let Some(redis::Value::Array(fields)) = pair.get(1) else {
                continue;
            };
            let text: Vec<String> = fields
                .iter()
                .map(|f| match f {
                    redis::Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
                    other => format!("{other:?}"),
                })
                .collect();
            let (chunks, _odd) = text.as_chunks::<2>();
            let pairs: Vec<(String, String)> = chunks
                .iter()
                .map(|c| (c[0].clone(), c[1].clone()))
                .collect();
            out.push((id, pairs));
        }
    }
    out
}

/// Pull the `payload` bytes out of a raw XRANGE reply.
fn decode_payload_from_rows(rows: &[redis::Value]) -> anyhow::Result<PingBatch> {
    let Some(redis::Value::Array(entry)) = rows.first() else {
        anyhow::bail!("entry vanished between XREADGROUP and XRANGE");
    };
    // entry = [id, [field, value, ...]]
    let Some(redis::Value::Array(fields)) = entry.get(1) else {
        anyhow::bail!("entry has no field array");
    };
    let mut payload: Option<Vec<u8>> = None;
    let (chunks, _odd) = fields.as_chunks::<2>();
    for chunk in chunks {
        if let (redis::Value::BulkString(k), v) = (&chunk[0], &chunk[1]) {
            if k == b"payload" {
                payload = Some(match v {
                    // redis-rs decodes stream field values as BulkString bytes;
                    // its Debug impl prints non-UTF8 payloads as binary-data(..).
                    redis::Value::BulkString(b) => b.clone(),
                    other => anyhow::bail!("payload field is not bytes: {other:?}"),
                });
            }
        }
    }
    let payload = payload.ok_or_else(|| anyhow::anyhow!("entry has no payload"))?;
    PingBatch::decode(&payload).map_err(|e| anyhow::anyhow!("{e}"))
}

fn init_tracing() {
    let fmt = tracing_subscriber::fmt::layer()
        .json()
        .with_target(false)
        .with_current_span(false)
        .with_span_list(false);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(fmt)
        .init();
}

fn require_env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("{key} must be set"))
}

async fn selftest() -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    println!("matcher selftest ok: bound {addr}");
    Ok(())
}
