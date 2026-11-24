//! `tabaani` matcher worker.
//!
//! Consumes ping batches from a Redis Stream consumer group, runs the verified
//! rail matcher from `transit-core`, and publishes vehicle state to Redis so
//! the api's live feed has something to serve.

use std::collections::HashMap;
use std::time::Duration;

use api::pb::{LiveFrame, PingBatch};
use api::state::StoredVehicle;
use redis::aio::MultiplexedConnection;
use transit_core::geometry::LineProject;
use transit_core::rail::{ObsFix, RailDecision, RailMatcher, TripTiming};

#[cfg(test)]
mod tests;

/// A line's static data, loaded once at boot.
pub struct Network {
    pub shapes: HashMap<String, LineProject>,
    pub trips: HashMap<String, Vec<TripTiming>>,
}

impl Network {
    /// Build from a GTFS directory (stops.txt + stop_times.txt).
    pub fn from_gtfs(dir: &std::path::Path) -> anyhow::Result<Self> {
        let mut shapes = HashMap::new();
        let mut trips: HashMap<String, Vec<TripTiming>> = HashMap::new();

        let stops_raw = std::fs::read_to_string(dir.join("stops.txt"))?;
        let mut stops: Vec<(String, f64, f64)> = Vec::new();
        for (i, line) in stops_raw.lines().skip(1).enumerate() {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 4 {
                anyhow::bail!("stops.txt line {} has {} fields", i + 2, f.len());
            }
            stops.push((f[0].to_string(), f[2].parse()?, f[3].parse()?));
        }

        let st_raw = std::fs::read_to_string(dir.join("stop_times.txt"))?;
        // trip_id -> [(time_s, dist_m)]
        let mut rows: Vec<(String, i64, f64)> = Vec::new();
        for (i, line) in st_raw.lines().skip(1).enumerate() {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 6 {
                anyhow::bail!("stop_times.txt line {} has {} fields", i + 2, f.len());
            }
            let t = parse_time(f[1]).ok_or_else(|| anyhow::anyhow!("bad time {}", f[1]))?;
            rows.push((f[0].to_string(), t, f[5].trim().parse()?));
        }
        rows.sort_by_key(|(_, t, _)| *t);
        let mut by_trip: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for (trip, t, d) in rows {
            by_trip.entry(trip).or_default().push((t, d));
        }
        for (trip, profile) in by_trip {
            let start_s = profile.first().map(|p| p.0).unwrap_or(0);
            trips.entry("tgm".into()).or_default().push(TripTiming {
                trip_id: trip,
                start_s,
                profile,
            });
        }

        // The pilot seed is one route; build its shape from the stop geometry.
        let coords: Vec<(f64, f64)> = stops.iter().map(|(_, lat, lon)| (*lon, *lat)).collect();
        if coords.len() >= 2 {
            shapes.insert("tgm".to_string(), LineProject::new(&coords));
        }
        Ok(Self { shapes, trips })
    }

    pub fn routes(&self) -> Vec<String> {
        self.shapes.keys().cloned().collect()
    }
}

fn parse_time(s: &str) -> Option<i64> {
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let sec: i64 = it.next()?.trim().parse().ok()?;
    Some(h * 3600 + m * 60 + sec)
}

/// Session state held in Redis between batches.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    pub token: String,
    pub route_id: Option<String>,
    pub obs: Vec<(i64, f64)>,
    pub last_seen_s: i64,
    pub trip_id: Option<String>,
    pub delay_s: f64,
}

/// Match one session's accumulated observations against every known route,
/// returning the best decision (highest confidence identified trip, else the
/// most plausible ambiguity).
pub fn decide(network: &Network, state: &SessionState) -> Option<RouteDecision> {
    let mut best: Option<RouteDecision> = None;
    for route_id in network.routes() {
        let Some(shape) = network.shapes.get(&route_id) else {
            continue;
        };
        let trips = match network.trips.get(&route_id) {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        // The stored observations are already along-route metres for this
        // route, because they were projected when the fix was accepted.
        let obs: Vec<ObsFix> = state
            .obs
            .iter()
            .map(|(t, s)| ObsFix {
                t_s: *t,
                s_m: *s,
                accuracy_m: 5.0,
            })
            .collect();
        let now_s = state.last_seen_s;
        let decision = match_route(shape, trips, &obs, now_s);
        let candidate = RouteDecision {
            route_id,
            decision: decision.clone(),
        };
        let better = match (&best, &decision) {
            (None, _) => true,
            (
                Some(RouteDecision {
                    decision: RailDecision::Identified { confidence: bc, .. },
                    ..
                }),
                RailDecision::Identified { confidence: c, .. },
            ) => c > bc,
            (Some(_), RailDecision::Ambiguous { .. }) => false,
            (
                Some(RouteDecision {
                    decision: RailDecision::Insufficient { .. },
                    ..
                }),
                _,
            ) => true,
            _ => false,
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub route_id: String,
    pub decision: RailDecision,
}

/// Convert absolute fixes into along-route observations for one route.
pub fn project_fixes(shape: &LineProject, fixes: &[AbsFix]) -> Vec<ObsFix> {
    fixes
        .iter()
        .filter_map(|f| {
            shape.project(f.lon(), f.lat()).map(|p| ObsFix {
                t_s: f.t_s,
                s_m: p.s_m,
                accuracy_m: f.accuracy_m,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub struct AbsFix {
    pub t_s: i64,
    pub lat_e7: i64,
    pub lon_e7: i64,
    pub accuracy_m: f64,
}

impl AbsFix {
    pub fn lat(&self) -> f64 {
        self.lat_e7 as f64 / 1e7
    }
    pub fn lon(&self) -> f64 {
        self.lon_e7 as f64 / 1e7
    }
}

/// Delta-decode a `PingBatch` payload into absolute fixes.
///
/// Wire convention (see `Fix`): the first fix in each batch carries absolute
/// lat/lon/timestamp; every later fix carries deltas from its predecessor. So
/// decoding is a plain running sum with no out-of-band origin required.
pub fn absolute_fixes(batch: &PingBatch) -> Vec<AbsFix> {
    let mut out = Vec::with_capacity(batch.fixes.len());
    let mut lat = 0i64;
    let mut lon = 0i64;
    let mut ts = 0i64;
    for f in &batch.fixes {
        lat += f.d_lat_e7 as i64;
        lon += f.d_lon_e7 as i64;
        ts += f.d_ts_ms as i64;
        out.push(AbsFix {
            t_s: ts.div_euclid(1000),
            lat_e7: lat,
            lon_e7: lon,
            accuracy_m: f.accuracy_dm as f64 / 10.0,
        });
    }
    out
}

/// Publish a decided vehicle to Redis live state and fan it out on pub/sub.
pub async fn publish(conn: &mut MultiplexedConnection, v: &StoredVehicle) -> anyhow::Result<()> {
    let key = format!("veh:{}", v.vehicle_id);
    let json = serde_json::to_string(v)?;
    // SET keeps the vehicle readable, EXPIRE bounds staleness, SADD indexes it.
    let set_res: String = redis::cmd("SET")
        .arg(&key)
        .arg(&json)
        .query_async(conn)
        .await?;
    tracing::debug!(vehicle = %v.vehicle_id, set = %set_res, "live state written");
    let exp: bool = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(300)
        .query_async(conn)
        .await?;
    let added: i64 = redis::cmd("SADD")
        .arg("veh:ids")
        .arg(&v.vehicle_id)
        .query_async(conn)
        .await?;
    // Re-read on the same connection so the log reflects what the api will see.
    let members: Vec<String> = redis::cmd("SMEMBERS")
        .arg("veh:ids")
        .query_async(conn)
        .await
        .unwrap_or_default();
    tracing::info!(
        vehicle = %v.vehicle_id,
        expire = exp,
        sadd_added = added,
        members = ?members,
        "live state written"
    );
    let frame = LiveFrame {
        vehicles: vec![v.clone().into_state()],
    };
    let _: () = redis::cmd("PUBLISH")
        .arg("live:all")
        .arg(frame.encode())
        .query_async(conn)
        .await?;
    Ok(())
}

/// Run the matcher decision for one route and return the best outcome.
pub fn match_route(
    shape: &LineProject,
    trips: &[TripTiming],
    obs: &[ObsFix],
    now_s: i64,
) -> RailDecision {
    let matcher = RailMatcher::new(shape);
    matcher.decide(obs, now_s, trips)
}

/// Idle backoff when the stream is empty.
#[allow(dead_code)]
const POLL_IDLE: Duration = Duration::from_millis(200);
