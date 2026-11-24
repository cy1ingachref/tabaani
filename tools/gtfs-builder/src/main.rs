//! `gtfs-builder` — turn a Tabaani line seed (`data/tab_seed.json`) into a
//! versioned GTFS dataset with `shape_dist_traveled` on every stop_time.
//!
//! The shape is the polyline through the stop coordinates (real track
//! centerlines arrive with the OSM step in Phase 1). `shape_dist_traveled` is
//! the distance along that polyline, which is the axis the rail matcher runs
//! on (plan §5.3).
//!
//! Output: `<out>/<version>/{agency,stops,routes,trips,stop_times,shapes,calendar,frequencies}.txt`
//! plus a `manifest.json`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use transit_core::geometry::LineProject;

#[derive(Debug, Deserialize)]
struct Seed {
    dataset: DatasetMeta,
    agency: Agency,
    route: RouteSeed,
}

#[derive(Debug, Deserialize)]
struct DatasetMeta {
    version: String,
    name: String,
    source: String,
    note: String,
}

#[derive(Debug, Deserialize)]
struct Agency {
    id: String,
    name: String,
    timezone: String,
    #[serde(default)]
    lang: String,
}

#[derive(Debug, Deserialize)]
struct StopSeed {
    id: String,
    name: String,
    lat: f64,
    lon: f64,
}

#[derive(Debug, Deserialize)]
struct RouteSeed {
    id: String,
    short_name: String,
    long_name: String,
    /// Kept in the seed for documentation and future mode-specific logic.
    #[allow(dead_code)]
    mode: String,
    route_type: i64,
    color: String,
    stops: Vec<StopSeed>,
    departures: Vec<String>,
    cruise_speed_mps: f64,
    dwell_s: i64,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let seed_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/tab_seed.json"));
    let out_root = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/gtfs/build"));

    let seed_raw = std::fs::read_to_string(&seed_path)
        .with_context(|| format!("reading seed {}", seed_path.display()))?;
    let seed: Seed =
        serde_json::from_str(&seed_raw).context("parsing seed JSON (schema mismatch)")?;

    if seed.route.stops.len() < 2 {
        anyhow::bail!("a route needs at least two stops");
    }

    // Build the shape and the along-track distance of every stop.
    let coords: Vec<(f64, f64)> = seed.route.stops.iter().map(|s| (s.lon, s.lat)).collect();
    let shape = LineProject::new(&coords);
    if shape.is_empty() {
        anyhow::bail!("degenerate route shape");
    }
    let mut stop_s: BTreeMap<String, f64> = BTreeMap::new();
    for s in &seed.route.stops {
        let p = shape
            .project(s.lon, s.lat)
            .ok_or_else(|| anyhow::anyhow!("stop {} did not project", s.id))?;
        stop_s.insert(s.id.clone(), p.s_m);
    }

    let version = sanitize(&seed.dataset.version);
    let dir = out_root.join(&version);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    write_agency(&dir, &seed.agency)?;
    write_stops(&dir, &seed.route.stops)?;
    write_routes(&dir, &seed.route)?;
    let trips = build_trips(&seed.route, &stop_s)?;
    write_trips(&dir, &trips)?;
    write_stop_times(&dir, &trips)?;
    write_shapes(&dir, &seed.route)?;
    write_calendar(&dir)?;
    write_manifest(&dir, &seed, shape.length_m(), &trips)?;

    println!(
        "gtfs-builder: wrote {} ({} stops, {} trips, {:.2} km)",
        dir.display(),
        seed.route.stops.len(),
        trips.len(),
        shape.length_m() / 1000.0
    );
    Ok(())
}

fn sanitize(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A trip: id, departure seconds after midnight, and per-stop times + distances.
struct Trip {
    id: String,
    start_s: i64,
    stop_times: Vec<(String, i64, f64)>, // (stop_id, time_s, shape_dist_m)
}

fn build_trips(route: &RouteSeed, stop_s: &BTreeMap<String, f64>) -> Result<Vec<Trip>> {
    let mut trips = Vec::new();
    for (i, dep) in route.departures.iter().enumerate() {
        let start_s =
            parse_hhmm(dep).ok_or_else(|| anyhow::anyhow!("bad departure time {dep:?}"))?;
        let mut stop_times = Vec::with_capacity(route.stops.len());
        let mut t = start_s;
        for k in 0..route.stops.len() {
            let s = &route.stops[k];
            let s_m = *stop_s
                .get(&s.id)
                .ok_or_else(|| anyhow::anyhow!("stop {} has no distance", s.id))?;
            stop_times.push((s.id.clone(), t, s_m));
            if k + 1 < route.stops.len() {
                let next = &route.stops[k + 1];
                let next_m = *stop_s
                    .get(&next.id)
                    .ok_or_else(|| anyhow::anyhow!("stop {} has no distance", next.id))?;
                let leg = (next_m - s_m).max(0.0);
                t += (leg / route.cruise_speed_mps).round() as i64;
                t += route.dwell_s;
            }
        }
        trips.push(Trip {
            id: format!("{}-{:03}", route.id, i + 1),
            start_s,
            stop_times,
        });
    }
    Ok(trips)
}

fn parse_hhmm(s: &str) -> Option<i64> {
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(h * 3600 + m * 60)
}

fn fmt_hhmm(mut s: i64) -> String {
    // GTFS permits >24:00:00 for after-midnight service.
    let h = s / 3600;
    s %= 3600;
    format!("{:02}:{:02}:{:02}", h, s / 60, s % 60)
}

fn write_csv(dir: &Path, name: &str, header: &str, rows: &[String]) -> Result<()> {
    let path = dir.join(name);
    let mut body = String::new();
    body.push_str(header);
    body.push('\n');
    for r in rows {
        body.push_str(r);
        body.push('\n');
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn row(fields: &[String]) -> String {
    fields
        .iter()
        .map(|f| csv_escape(f))
        .collect::<Vec<_>>()
        .join(",")
}

fn write_agency(dir: &Path, a: &Agency) -> Result<()> {
    write_csv(
        dir,
        "agency.txt",
        "agency_id,agency_name,agency_timezone,agency_lang",
        &[row(&[
            a.id.clone(),
            a.name.clone(),
            a.timezone.clone(),
            if a.lang.is_empty() {
                "fr".into()
            } else {
                a.lang.clone()
            },
        ])],
    )
}

fn write_stops(dir: &Path, stops: &[StopSeed]) -> Result<()> {
    let rows: Vec<String> = stops
        .iter()
        .map(|s| {
            row(&[
                s.id.clone(),
                s.name.clone(),
                format!("{:.6}", s.lat),
                format!("{:.6}", s.lon),
            ])
        })
        .collect();
    write_csv(
        dir,
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon",
        &rows,
    )
}

fn write_routes(dir: &Path, r: &RouteSeed) -> Result<()> {
    write_csv(
        dir,
        "routes.txt",
        "route_id,agency_id,route_short_name,route_long_name,route_type,route_color",
        &[row(&[
            r.id.clone(),
            "transtu".into(),
            r.short_name.clone(),
            r.long_name.clone(),
            r.route_type.to_string(),
            r.color.clone(),
        ])],
    )
}

fn write_trips(dir: &Path, trips: &[Trip]) -> Result<()> {
    let rows: Vec<String> = trips
        .iter()
        .map(|t| {
            row(&[
                t.id.clone(),
                "tgm".into(),
                "wd".into(),
                "0".into(),
                "tgm".into(),
            ])
        })
        .collect();
    write_csv(
        dir,
        "trips.txt",
        "route_id,service_id,trip_id,trip_direction_id,shape_id",
        &rows,
    )
}

fn write_stop_times(dir: &Path, trips: &[Trip]) -> Result<()> {
    let rows: Vec<String> = trips
        .iter()
        .flat_map(|t| {
            t.stop_times
                .iter()
                .enumerate()
                .map(move |(k, (sid, ts, s_m))| {
                    row(&[
                        t.id.clone(),
                        fmt_hhmm(*ts),
                        fmt_hhmm(*ts),
                        sid.clone(),
                        (k + 1).to_string(),
                        format!("{s_m:.1}"),
                    ])
                })
        })
        .collect();
    write_csv(
        dir,
        "stop_times.txt",
        "trip_id,arrival_time,departure_time,stop_id,stop_sequence,shape_dist_traveled",
        &rows,
    )
}

fn write_shapes(dir: &Path, r: &RouteSeed) -> Result<()> {
    let rows: Vec<String> = r
        .stops
        .iter()
        .enumerate()
        .map(|(k, s)| {
            row(&[
                r.id.clone(),
                format!("{:.6}", s.lon),
                format!("{:.6}", s.lat),
                k.to_string(),
                "0".into(),
            ])
        })
        .collect();
    write_csv(
        dir,
        "shapes.txt",
        "shape_id,shape_pt_lon,shape_pt_lat,shape_pt_sequence,shape_dist_traveled",
        &rows,
    )
}

fn write_calendar(dir: &Path) -> Result<()> {
    write_csv(
        dir,
        "calendar.txt",
        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date",
        &[row(&[
            "wd".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "20260101".into(),
            "20261231".into(),
        ])],
    )
}

fn write_manifest(dir: &Path, seed: &Seed, length_m: f64, trips: &[Trip]) -> Result<()> {
    let first_dep = trips.first().map(|t| t.start_s).unwrap_or(0);
    let last_arr = trips
        .last()
        .and_then(|t| t.stop_times.last())
        .map(|(_, ts, _)| *ts)
        .unwrap_or(0);
    // Manifest strings are escaped for JSON; `json_str` adds the surrounding
    // quotes itself, so pass the raw text.
    let manifest = format!(
        "{{\n  \"version\": {},\n  \"name\": {},\n  \"source\": {},\n  \"note\": {},\n  \"route_id\": {},\n  \"length_m\": {:.1},\n  \"trips\": {},\n  \"first_departure\": \"{}\",\n  \"last_arrival\": \"{}\"\n}}\n",
        json_str(&seed.dataset.version),
        json_str(&seed.dataset.name),
        json_str(&seed.dataset.source),
        json_str(&seed.dataset.note),
        json_str(&seed.route.id),
        length_m,
        trips.len(),
        fmt_hhmm(first_dep),
        fmt_hhmm(last_arr),
    );
    std::fs::write(dir.join("manifest.json"), manifest).context("writing manifest.json")?;
    Ok(())
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[allow(dead_code)]
fn unused_write_helper() {
    // Keep the `std::fmt::Write` import meaningful for future manifest tweaks.
    let _ = write!(String::new(), "");
}
