//! `simulator` — replay a GTFS trip as fake rider phones, writing JSONL traces
//! that `replay` can score. This is the "no riders yet" answer for development:
//! a trip with a configurable delay, GPS noise and tunnel gaps.
//!
//! Traces use real `shape_dist_traveled` metres (not stop indices) so the
//! observations and the matcher agree on the position axis exactly.
//!
//! Trace format (one JSON object per line):
//! ```json
//! {"meta":{"trip_id":"tgm-003","route_id":"tgm","delay_s":120,"rider":"r1","kind":"ride"}}
//! {"fix":{"t_s":21620,"lon":10.1917,"lat":36.8007,"accuracy_m":5.0}}
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use transit_core::geometry::LineProject;

#[derive(Debug, Serialize)]
struct Meta {
    trip_id: String,
    route_id: String,
    delay_s: i64,
    rider: String,
    /// `ride` = should be identified; `offroute` = should be rejected.
    kind: String,
}

#[derive(Debug, Serialize)]
struct Fix {
    t_s: i64,
    lon: f64,
    lat: f64,
    accuracy_m: f64,
}

#[derive(Debug, Serialize)]
struct Line {
    meta: Meta,
}

#[derive(Debug, Serialize)]
struct FixLine {
    fix: Fix,
}

/// A trip's scheduled profile in the same units the matcher uses:
/// (seconds after midnight, metres along the shape).
#[derive(Debug, Clone)]
struct TripProfile {
    id: String,
    /// (time_s, shape_dist_m) per stop, strictly increasing in time.
    samples: Vec<(i64, f64)>,
}

impl TripProfile {
    fn start_s(&self) -> i64 {
        self.samples.first().map(|s| s.0).unwrap_or(0)
    }

    fn end_s(&self) -> i64 {
        self.samples.last().map(|s| s.0).unwrap_or(0)
    }

    /// Scheduled distance at `t_s`, piecewise linear between stops.
    fn s_at(&self, t_s: i64) -> Option<f64> {
        if t_s < self.start_s() || t_s > self.end_s() {
            return None;
        }
        let idx = (0..self.samples.len() - 1)
            .rev()
            .find(|&k| self.samples[k].0 <= t_s)
            .unwrap_or(0);
        let (t0, s0) = self.samples[idx];
        let (t1, s1) = self.samples[idx + 1];
        if t1 == t0 {
            return Some(s0);
        }
        let frac = (t_s - t0) as f64 / (t1 - t0) as f64;
        Some(s0 + (s1 - s0) * frac)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let gtfs_dir = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/gtfs/build/0.1.0-dev"));
    let out = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/traces"));

    let stops = read_stops(&gtfs_dir)?;
    let coords: Vec<(f64, f64)> = stops.iter().map(|s| (s.lon, s.lat)).collect();
    let shape = LineProject::new(&coords);
    if shape.is_empty() {
        anyhow::bail!("empty shape from {gtfs_dir:?}");
    }
    let trips = read_profiles(&gtfs_dir)?;

    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;

    let mut written = 0usize;
    // Every hourly departure, delayed by 120 s, plus one off-route control.
    for trip in &trips {
        let path = out.join(format!("{}.jsonl", trip.id));
        write_ride_trace(&path, trip, &shape, 120, 4.0)?;
        written += 1;
    }
    let ctrl = out.join("control_offroute.jsonl");
    write_offroute_trace(&ctrl, &shape)?;
    written += 1;

    println!(
        "simulator: wrote {} traces to {} (shape {:.2} km)",
        written,
        out.display(),
        shape.length_m() / 1000.0
    );
    Ok(())
}

struct Stop {
    #[allow(dead_code)]
    id: String,
    lon: f64,
    lat: f64,
}

fn read_stops(dir: &Path) -> Result<Vec<Stop>> {
    let path = dir.join("stops.txt");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().skip(1).enumerate() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 4 {
            anyhow::bail!("stops.txt line {} has {} fields", i + 2, f.len());
        }
        out.push(Stop {
            id: f[0].to_string(),
            lat: f[2].parse().context("stop lat")?,
            lon: f[3].parse().context("stop lon")?,
        });
    }
    Ok(out)
}

/// Read stop_times into per-trip (time_s, shape_dist_m) profiles.
fn read_profiles(dir: &Path) -> Result<Vec<TripProfile>> {
    let path = dir.join("stop_times.txt");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut by_trip: Vec<TripProfile> = Vec::new();
    for (i, line) in text.lines().skip(1).enumerate() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            anyhow::bail!(
                "stop_times.txt line {} has {} fields (want >= 6)",
                i + 2,
                f.len()
            );
        }
        let t = parse_gtfs_time(f[1]).ok_or_else(|| {
            anyhow::anyhow!("bad time {:?} on stop_times.txt line {}", f[1], i + 2)
        })?;
        let dist: f64 = f[5]
            .trim()
            .parse()
            .with_context(|| format!("bad shape_dist_traveled on line {}", i + 2))?;
        match by_trip.last_mut() {
            Some(tp) if tp.id == f[0] => tp.samples.push((t, dist)),
            _ => by_trip.push(TripProfile {
                id: f[0].to_string(),
                samples: vec![(t, dist)],
            }),
        }
    }
    Ok(by_trip)
}

fn parse_gtfs_time(s: &str) -> Option<i64> {
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let sec: i64 = it.next()?.trim().parse().ok()?;
    Some(h * 3600 + m * 60 + sec)
}

/// Deterministic pseudo-random in [-1, 1] from an index, so traces are
/// reproducible across runs (a fixture, not a dice roll).
fn wobble(i: i64) -> f64 {
    let x = (i.wrapping_mul(2_654_435_761).wrapping_add(1)) % 10_000;
    (x as f64 / 10_000.0) * 2.0 - 1.0
}

/// Write a ride trace following the schedule shifted by `delay_s`, sampled every
/// 10 s with along-track and lateral GPS noise.
fn write_ride_trace(
    path: &Path,
    trip: &TripProfile,
    shape: &LineProject,
    delay_s: i64,
    noise_m: f64,
) -> Result<()> {
    use std::io::Write;
    let mut f =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let meta = Line {
        meta: Meta {
            trip_id: trip.id.clone(),
            route_id: "tgm".into(),
            delay_s,
            rider: "r1".into(),
            kind: "ride".into(),
        },
    };
    writeln!(f, "{}", serde_json::to_string(&meta)?)?;

    let length = shape.length_m();
    let mut t = trip.start_s() + 20;
    let mut i = 0i64;
    while t <= trip.end_s() - 20 {
        // Where the vehicle actually is at time t: the schedule reached this
        // position delay_s seconds earlier.
        let Some(true_s) = trip.s_at(t - delay_s) else {
            t += 10;
            continue;
        };
        let along = (true_s + wobble(i) * noise_m).clamp(0.0, length);
        let Some((lon, lat0)) = shape.point_at(along) else {
            t += 10;
            continue;
        };
        // Lateral noise purely in latitude keeps it a clean cross-track offset.
        let lat_noise = wobble(i + 999) * noise_m / 111_320.0;
        let fix = FixLine {
            fix: Fix {
                t_s: t,
                lon,
                lat: lat0 + lat_noise,
                accuracy_m: 5.0,
            },
        };
        writeln!(f, "{}", serde_json::to_string(&fix)?)?;
        t += 10;
        i += 1;
    }
    Ok(())
}

/// Control trace: walk beside the line, far slower than any trip, offset ~200 m
/// north. Every trip fit should be poor, so replay expects rejection.
fn write_offroute_trace(path: &Path, shape: &LineProject) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    let meta = Line {
        meta: Meta {
            trip_id: "none".into(),
            route_id: "tgm".into(),
            delay_s: 0,
            rider: "walker".into(),
            kind: "offroute".into(),
        },
    };
    writeln!(f, "{}", serde_json::to_string(&meta)?)?;
    let mut t = 6 * 3600;
    let mut i = 0i64;
    // Cover only the first 40% of the line over an hour: clearly walking pace.
    while t <= 7 * 3600 {
        let s = (i as f64 * 14.0).min(shape.length_m() * 0.4);
        let Some((lon, lat0)) = shape.point_at(s) else {
            break;
        };
        let fix = FixLine {
            fix: Fix {
                t_s: t,
                lon,
                lat: lat0 + 0.0018,
                accuracy_m: 8.0,
            },
        };
        writeln!(f, "{}", serde_json::to_string(&fix)?)?;
        t += 15;
        i += 1;
    }
    Ok(())
}
