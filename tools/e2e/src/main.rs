//! `e2e` — live end-to-end check of the ingest -> matcher -> live pipeline.
//!
//! Connects to a running api over WebSocket, streams a simulated rider's ping
//! batches exactly as the mobile client would (delta-encoded protobuf), then
//! subscribes to `/v1/live` and waits for a vehicle to appear. Prints the
//! measured ping->visible latency and exits non-zero if the deadline is missed.
//!
//! Requires: redis on $REDIS_URL, api on $API_WS, matcher on $MATCHER_GTFS_DIR.
//! Run via `make e2e`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use api::pb::{Fix, PingBatch};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const API_WS_DEFAULT: &str = "ws://127.0.0.1:8080";

#[tokio::main]
async fn main() -> Result<()> {
    let api_ws = std::env::var("API_WS").unwrap_or_else(|_| API_WS_DEFAULT.into());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());

    // Start from a clean stream so the run measures only this rider. The
    // matcher must be consuming BEFORE we delete, because its consumer group is
    // anchored at 0 and recreating the stream would orphan it. Destroy the
    // group as well so a fresh one is created on the next entries.
    let client = redis::Client::open(redis_url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let _: () = redis::cmd("DEL")
        .arg("pings:{default}")
        .arg("veh:ids")
        .query_async(&mut conn)
        .await?;
    println!("e2e: cleared pings:{{default}} and veh:ids");

    let (mut sink, _resp) = tokio_tungstenite::connect_async(format!("{api_ws}/v1/ingest"))
        .await
        .context("connecting to /v1/ingest")?
        .0
        .split();
    println!("e2e: connected to {api_ws}/v1/ingest");

    // Rider boards tgm-004 (08:00) 120 s late, as in tools/simulator.
    let (fixes, base_t_s, base_lat_e7, base_lon_e7) = rider_trace();
    let batches = chunk_into_batches(&fixes, base_t_s, base_lat_e7, base_lon_e7, 20);
    println!(
        "e2e: sending {} batches ({} fixes)",
        batches.len(),
        fixes.len()
    );

    // Subscribe BEFORE publishing. The matcher emits a delta on each batch it
    // consumes; connecting afterwards would only catch later frames and could
    // miss the moment the vehicle first appears.
    let (mut live_sink, mut live_stream) =
        tokio_tungstenite::connect_async(format!("{api_ws}/v1/live"))
            .await
            .context("connecting to /v1/live")?
            .0
            .split();
    let _ = &mut live_sink;
    println!("e2e: subscribed to {api_ws}/v1/live");

    let started = Instant::now();
    for b in &batches {
        let payload = b.encode();
        sink.send(Message::Binary(payload.into()))
            .await
            .context("sending batch")?;
        // Roughly real time: 20 fixes at 10 s each is a 200 s ride; speed it up
        // so the check stays fast while still exercising the pipeline.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let sent_in = started.elapsed();
    println!("e2e: all batches sent in {sent_in:?}");

    // Now drain frames until a vehicle appears.
    let deadline = Duration::from_secs(30);
    let mut seen = Vec::new();
    loop {
        let elapsed = started.elapsed();
        if elapsed > deadline {
            anyhow::bail!(
                "no live vehicle within {deadline:?} (elapsed {:?}); saw {}",
                elapsed,
                seen.len()
            );
        }
        let next = tokio::time::timeout(Duration::from_millis(500), live_stream.next()).await;
        match next {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                let frame = api::pb::LiveFrame::decode(&bytes)?;
                for v in frame.vehicles {
                    if !seen.contains(&v.vehicle_id) {
                        seen.push(v.vehicle_id.clone());
                        println!(
                            "e2e: LIVE {} route={} trip={} delay={:.0}s conf={:?} after {:?}",
                            v.vehicle_id,
                            v.route_id,
                            v.trip_id,
                            v.delay_s,
                            v.confidence,
                            started.elapsed()
                        );
                    }
                }
                if !seen.is_empty() {
                    break;
                }
            }
            Ok(Some(Ok(other))) => {
                println!("e2e: non-binary frame: {other:?}");
            }
            Ok(Some(Err(e))) => anyhow::bail!("live socket error: {e}"),
            Ok(None) => anyhow::bail!("live socket closed"),
            Err(_) => continue,
        }
    }

    println!("\ne2e: PASS — vehicle visible on /v1/live");
    Ok(())
}

/// Absolute fixes for a rider on tgm-004, delayed 120 s, sampled every 10 s.
///
/// The rider's position follows the GTFS `stop_times` profile for that trip
/// (piecewise linear between stops), NOT a constant-speed guess. Reconstructing
/// it from the same timetable is what makes this a fair end-to-end check: the
/// matcher should recover the trip and the injected delay.
fn rider_trace() -> (Vec<(i64, f64, f64)>, i64, i64, i64) {
    let coords: Vec<(f64, f64)> = vec![
        (10.19174, 36.80069),
        (10.29245, 36.81418),
        (10.30194, 36.81805),
        (10.30560, 36.81984),
        (10.30877, 36.82420),
        (10.31155, 36.82878),
        (10.31387, 36.83195),
        (10.31661, 36.83617),
        (10.31910, 36.84154),
        (10.32185, 36.84613),
        (10.32558, 36.85012),
        (10.32892, 36.85339),
        (10.33494, 36.86165),
        (10.33744, 36.86659),
        (10.34213, 36.87045),
        (10.33967, 36.87574),
        (10.33349, 36.88298),
        (10.33345, 36.88304),
    ];
    let shape = transit_core::geometry::LineProject::new(&coords);
    let length = shape.length_m();

    // Read tgm-004's own stop times so the simulated ride matches the schedule.
    let gtfs = std::env::var("GTFS_DIR").unwrap_or_else(|_| "data/gtfs/build/0.1.0-dev".into());
    let text = std::fs::read_to_string(format!("{gtfs}/stop_times.txt")).expect("stop_times.txt");
    let mut samples: Vec<(i64, f64)> = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 || f[0] != "tgm-004" {
            continue;
        }
        let t = parse_hhmm(f[1]).expect("valid time");
        let d: f64 = f[5].trim().parse().expect("valid distance");
        samples.push((t, d));
    }
    assert!(samples.len() >= 2, "tgm-004 not found in {gtfs}");

    let delay_s = 120i64;
    let start = samples[0].0 + 20;
    let end = samples[samples.len() - 1].0 - 20;
    let mut out = Vec::new();
    let mut t = start;
    let mut i = 0i64;
    while t <= end {
        // Scheduled distance at the true (delayed) time.
        let sched = interpolate(&samples, t - delay_s).unwrap_or(0.0);
        let wobble = (((i * 2_654_435_761) % 1000) as f64 / 1000.0 * 2.0 - 1.0) * 3.0;
        let s = (sched + wobble).clamp(0.0, length);
        let Some((lon, lat)) = shape.point_at(s) else {
            t += 10;
            i += 1;
            continue;
        };
        out.push((t, lat, lon));
        t += 10;
        i += 1;
    }
    let first = out[0];
    // base_* is the origin delta 0 refers to; the decoder sums from zero, so 0
    // is correct because the first fix carries absolute coordinates.
    (out, first.0, 0, 0)
}

fn parse_hhmm(s: &str) -> Option<i64> {
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let sec: i64 = it.next()?.trim().parse().ok()?;
    Some(h * 3600 + m * 60 + sec)
}

fn interpolate(samples: &[(i64, f64)], t: i64) -> Option<f64> {
    if t < samples[0].0 || t > samples[samples.len() - 1].0 {
        return None;
    }
    let idx = (0..samples.len() - 1)
        .rev()
        .find(|&k| samples[k].0 <= t)
        .unwrap_or(0);
    let (t0, s0) = samples[idx];
    let (t1, s1) = samples[idx + 1];
    if t1 == t0 {
        return Some(s0);
    }
    let frac = (t - t0) as f64 / (t1 - t0) as f64;
    Some(s0 + (s1 - s0) * frac)
}

/// Delta-encode absolute fixes into `PingBatch` messages.
///
/// Wire convention: each batch's FIRST fix carries absolute lat/lon/timestamp,
/// later fixes carry deltas from their predecessor (see `absolute_fixes`).
fn chunk_into_batches(
    fixes: &[(i64, f64, f64)],
    _base_t_s: i64,
    base_lat_e7: i64,
    base_lon_e7: i64,
    per_batch: usize,
) -> Vec<PingBatch> {
    let mut out = Vec::new();
    for (bi, chunk) in fixes.chunks(per_batch).enumerate() {
        let mut prev_lat = base_lat_e7;
        let mut prev_lon = base_lon_e7;
        let mut prev_ts_ms = 0i64;
        let mut proto = Vec::with_capacity(chunk.len());
        for &(t_s, lat, lon) in chunk {
            let lat_e7 = (lat * 1e7) as i64;
            let lon_e7 = (lon * 1e7) as i64;
            let ts_ms = t_s * 1000;
            proto.push(Fix {
                d_lat_e7: (lat_e7 - prev_lat) as i32,
                d_lon_e7: (lon_e7 - prev_lon) as i32,
                d_ts_ms: (ts_ms - prev_ts_ms) as u32,
                accuracy_dm: 50,
                speed_cms: 0,
                bearing_deg: 0,
            });
            prev_lat = lat_e7;
            prev_lon = lon_e7;
            prev_ts_ms = ts_ms;
        }
        out.push(PingBatch {
            // One rider = one session token, so the matcher accumulates the whole
            // ride. Rotating tokens mid-ride would split it into fragments that
            // never individually reach identification confidence.
            session_token: "e2e-rider".into(),
            seq_start: (bi as u32) * per_batch as u32,
            dataset_version: "0.1.0-dev".into(),
            fixes: proto,
            hint: None,
        });
    }
    out
}
