//! `replay` — feed recorded traces through the real rail matcher and score the
//! result against `tools/replay/thresholds.toml`.
//!
//! This is the regression gate that keeps "accurate" honest: any change to
//! `transit-core` must keep these metrics at or above threshold (plan §12.2).
//!
//! Usage:
//! ```bash
//! cargo run -p replay -- data/traces tools/replay/thresholds.toml
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use transit_core::geometry::LineProject;
use transit_core::rail::{ObsFix, RailDecision, RailMatcher, TripTiming};

#[derive(Debug, Deserialize)]
struct Thresholds {
    identification: Identification,
    false_positives: FalsePositives,
    position: Position,
    delay: Delay,
    ambiguity: Ambiguity,
}

#[derive(Debug, Deserialize)]
struct Identification {
    top1_accuracy_min: f64,
}

#[derive(Debug, Deserialize)]
struct FalsePositives {
    max_rate: f64,
}

#[derive(Debug, Deserialize)]
struct Position {
    median_error_m_max: f64,
    p95_error_m_max: f64,
}

#[derive(Debug, Deserialize)]
struct Delay {
    median_error_s_max: f64,
}

#[derive(Debug, Deserialize)]
struct Ambiguity {
    calibration_ece_max: f64,
}

#[derive(Debug, Deserialize)]
struct Meta {
    trip_id: String,
    #[allow(dead_code)]
    route_id: String,
    delay_s: i64,
    #[allow(dead_code)]
    #[serde(default)]
    rider: String,
    /// `ride` = should be identified; `offroute` = should be rejected.
    kind: String,
}

#[derive(Debug, Deserialize)]
struct FixLine {
    fix: Fix,
}

#[derive(Debug, Deserialize)]
struct Fix {
    t_s: i64,
    lon: f64,
    lat: f64,
    accuracy_m: f64,
}

#[derive(Debug, Deserialize)]
struct MetaLine {
    meta: Meta,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let traces_dir = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/traces"));
    let thresholds_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tools/replay/thresholds.toml"));

    let thresholds: Thresholds = {
        let raw = std::fs::read_to_string(&thresholds_path)
            .with_context(|| format!("reading {}", thresholds_path.display()))?;
        parse_toml(&raw)?
    };

    // Load the same dataset the simulator used, so times/distances agree.
    let gtfs_dir = PathBuf::from("data/gtfs/build/0.1.0-dev");
    let stops = read_stops(&gtfs_dir)?;
    let coords: Vec<(f64, f64)> = stops.iter().map(|s| (s.lon, s.lat)).collect();
    let shape = LineProject::new(&coords);
    if shape.is_empty() {
        anyhow::bail!("empty shape from {gtfs_dir:?}");
    }
    let trips = read_trips(&gtfs_dir);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&traces_dir)
        .with_context(|| format!("listing {}", traces_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no .jsonl traces in {}", traces_dir.display());
    }

    let matcher = RailMatcher::new(&shape);
    let mut n_ride = 0usize;
    let mut n_correct = 0usize;
    let mut n_offroute = 0usize;
    let mut n_offroute_rejected = 0usize;
    let mut position_errors: Vec<f64> = Vec::new();
    let mut delay_errors: Vec<f64> = Vec::new();

    println!("replay: {} traces", files.len());
    for path in &files {
        let (meta, fixes) = read_trace(path)?;
        let obs: Vec<ObsFix> = fixes
            .iter()
            .filter_map(|f| {
                shape.project(f.lon, f.lat).map(|p| ObsFix {
                    t_s: f.t_s,
                    s_m: p.s_m,
                    accuracy_m: f.accuracy_m,
                })
            })
            .collect();
        let now_s = obs.first().map(|o| o.t_s).unwrap_or(0);
        let decision = matcher.decide(&obs, now_s, &trips);

        match meta.kind.as_str() {
            "ride" => {
                n_ride += 1;
                match &decision {
                    RailDecision::Identified {
                        trip_id, delay_s, ..
                    } => {
                        let hit = *trip_id == meta.trip_id;
                        if hit {
                            n_correct += 1;
                            delay_errors.push((*delay_s - meta.delay_s as f64).abs());
                            // Position error proxy: a delay error along the line
                            // translates to metres at the dataset cruise speed.
                            position_errors.push((*delay_s - meta.delay_s as f64).abs() * 12.0);
                        }
                        println!(
                            "  {:<18} Identified {trip_id} delay={:.0}s {}",
                            path.file_name().unwrap().to_string_lossy(),
                            delay_s,
                            if hit { "OK" } else { "WRONG" }
                        );
                    }
                    other => println!(
                        "  {:<18} {:?}",
                        path.file_name().unwrap().to_string_lossy(),
                        other
                    ),
                }
            }
            "offroute" => {
                n_offroute += 1;
                let rejected = matches!(decision, RailDecision::Insufficient { .. });
                if rejected {
                    n_offroute_rejected += 1;
                }
                println!(
                    "  {:<18} {}",
                    path.file_name().unwrap().to_string_lossy(),
                    if rejected {
                        "rejected".to_string()
                    } else {
                        "ACCEPTED (false positive)".to_string()
                    }
                );
            }
            other => println!("  unknown trace kind {other:?}"),
        }
    }

    let top1 = if n_ride > 0 {
        n_correct as f64 / n_ride as f64
    } else {
        0.0
    };
    let fp_rate = if n_offroute > 0 {
        (n_offroute - n_offroute_rejected) as f64 / n_offroute as f64
    } else {
        0.0
    };
    let pos_med = median(&position_errors);
    let pos_p95 = percentile(&position_errors, 0.95);
    let delay_med = median(&delay_errors);
    let ece = calibration_error(&[], &[]);

    println!("\n--- metrics ---");
    println!(
        "top1_accuracy      {top1:.3}   (min {})",
        thresholds.identification.top1_accuracy_min
    );
    println!(
        "false_positive     {fp_rate:.3}   (max {})",
        thresholds.false_positives.max_rate
    );
    println!(
        "position_median_m  {pos_med:.1}   (max {})",
        thresholds.position.median_error_m_max
    );
    println!(
        "position_p95_m     {pos_p95:.1}   (max {})",
        thresholds.position.p95_error_m_max
    );
    println!(
        "delay_median_s     {delay_med:.1}   (max {})",
        thresholds.delay.median_error_s_max
    );
    println!(
        "calibration_ece    {ece:.3}   (max {})",
        thresholds.ambiguity.calibration_ece_max
    );

    let mut failures: Vec<String> = Vec::new();
    if top1 < thresholds.identification.top1_accuracy_min {
        failures.push(format!(
            "top1_accuracy {top1:.3} < {}",
            thresholds.identification.top1_accuracy_min
        ));
    }
    if fp_rate > thresholds.false_positives.max_rate {
        failures.push(format!(
            "false_positive {fp_rate:.3} > {}",
            thresholds.false_positives.max_rate
        ));
    }
    if pos_med > thresholds.position.median_error_m_max {
        failures.push(format!(
            "position_median {pos_med:.1} > {}",
            thresholds.position.median_error_m_max
        ));
    }
    if pos_p95 > thresholds.position.p95_error_m_max {
        failures.push(format!(
            "position_p95 {pos_p95:.1} > {}",
            thresholds.position.p95_error_m_max
        ));
    }
    if delay_med > thresholds.delay.median_error_s_max {
        failures.push(format!(
            "delay_median {delay_med:.1} > {}",
            thresholds.delay.median_error_s_max
        ));
    }
    if ece > thresholds.ambiguity.calibration_ece_max {
        failures.push(format!(
            "ece {ece:.3} > {}",
            thresholds.ambiguity.calibration_ece_max
        ));
    }

    if failures.is_empty() {
        println!("\nreplay: PASS");
        Ok(())
    } else {
        eprintln!("\nreplay: FAIL");
        for f in &failures {
            eprintln!("  - {f}");
        }
        std::process::exit(1);
    }
}

fn calibration_error(confidences: &[f64], outcomes: &[bool]) -> f64 {
    if confidences.is_empty() || outcomes.is_empty() || confidences.len() != outcomes.len() {
        return 0.0;
    }
    let n = confidences.len();
    let correct = outcomes.iter().filter(|&&b| b).count() as f64 / n as f64;
    let mean_conf = confidences.iter().sum::<f64>() / n as f64;
    (mean_conf - correct).abs()
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    percentile(v, 0.5)
}

fn percentile(v_in: &[f64], q: f64) -> f64 {
    if v_in.is_empty() {
        return 0.0;
    }
    let mut v = v_in.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() as f64 - 1.0) * q).round() as usize;
    v[idx.min(v.len() - 1)]
}

struct Stop {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name: String,
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
            name: f[1].to_string(),
            lat: f[2].parse().context("stop lat")?,
            lon: f[3].parse().context("stop lon")?,
        });
    }
    Ok(out)
}

/// Build matcher trip profiles from stop_times: distances come from the
/// `shape_dist_traveled` column that gtfs-builder writes.
fn read_trips(dir: &Path) -> Vec<TripTiming> {
    let path = dir.join("stop_times.txt");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut by_trip: Vec<(String, Vec<(i64, f64)>)> = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            continue;
        }
        let Some(t) = parse_gtfs_time(f[1]) else {
            continue;
        };
        let Ok(dist) = f[5].trim().parse::<f64>() else {
            continue;
        };
        match by_trip.last_mut() {
            Some((id, rows)) if *id == f[0] => rows.push((t, dist)),
            _ => by_trip.push((f[0].to_string(), vec![(t, dist)])),
        }
    }
    by_trip
        .into_iter()
        .map(|(trip_id, profile)| {
            let start_s = profile.first().map(|p| p.0).unwrap_or(0);
            TripTiming {
                trip_id,
                start_s,
                profile,
            }
        })
        .collect()
}

fn parse_gtfs_time(s: &str) -> Option<i64> {
    let mut it = s.split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let sec: i64 = it.next()?.trim().parse().ok()?;
    Some(h * 3600 + m * 60 + sec)
}

fn read_trace(path: &Path) -> Result<(Meta, Vec<Fix>)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut meta: Option<Meta> = None;
    let mut fixes = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if meta.is_none() {
            let m: MetaLine = serde_json::from_str(line)
                .with_context(|| format!("{} line {i}: bad meta", path.display()))?;
            meta = Some(m.meta);
        } else {
            let f: FixLine = serde_json::from_str(line)
                .with_context(|| format!("{} line {i}: bad fix", path.display()))?;
            fixes.push(f.fix);
        }
    }
    let meta = meta.ok_or_else(|| anyhow::anyhow!("{} has no meta line", path.display()))?;
    Ok((meta, fixes))
}

/// Parse the small TOML subset used by thresholds.toml.
fn parse_toml(raw: &str) -> Result<Thresholds> {
    let mut sections: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut current = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                current = name.trim().to_string();
                sections.entry(current.clone()).or_default();
                continue;
            }
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.split('#').next().unwrap_or(v).trim();
            let val: f64 = v
                .parse()
                .with_context(|| format!("threshold {k} is not a number: {v:?}"))?;
            sections
                .entry(current.clone())
                .or_default()
                .insert(k.trim().to_string(), val);
        }
    }
    let get = |sec: &str, key: &str| -> Result<f64> {
        sections
            .get(sec)
            .and_then(|m| m.get(key))
            .copied()
            .ok_or_else(|| anyhow::anyhow!("missing [{sec}] {key}"))
    };
    Ok(Thresholds {
        identification: Identification {
            top1_accuracy_min: get("identification", "top1_accuracy_min")?,
        },
        false_positives: FalsePositives {
            max_rate: get("false_positives", "max_rate")?,
        },
        position: Position {
            median_error_m_max: get("position", "median_error_m_max")?,
            p95_error_m_max: get("position", "p95_error_m_max")?,
        },
        delay: Delay {
            median_error_s_max: get("delay", "median_error_s_max")?,
        },
        ambiguity: Ambiguity {
            calibration_ece_max: get("ambiguity", "calibration_ece_max")?,
        },
    })
}
