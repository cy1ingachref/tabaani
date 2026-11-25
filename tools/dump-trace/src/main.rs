//! Dump the e2e rider trace as JSON for the Flutter client fixture.
use serde_json::json;
use std::fs;

fn main() {
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
    let text = fs::read_to_string("data/gtfs/build/0.1.0-dev/stop_times.txt").unwrap();
    let mut samples: Vec<(i64, f64)> = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 || f[0] != "tgm-004" {
            continue;
        }
        let t = parse_hhmm(f[1]).unwrap();
        let d: f64 = f[5].trim().parse().unwrap();
        samples.push((t, d));
    }
    let delay = 120i64;
    let start = samples[0].0 + 20;
    let end = samples[samples.len() - 1].0 - 20;
    let mut out = Vec::new();
    let (mut t, mut i) = (start, 0i64);
    while t <= end {
        let sched = interpolate(&samples, t - delay).unwrap_or(0.0);
        let wobble = (((i * 2_654_435_761) % 1000) as f64 / 1000.0 * 2.0 - 1.0) * 3.0;
        let s = (sched + wobble).clamp(0.0, length);
        if let Some((lon, lat)) = shape.point_at(s) {
            out.push(json!([lat, lon]));
        }
        t += 10;
        i += 1;
    }
    println!("{}", json!(out));
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
    Some(s0 + (s1 - s0) * (t - t0) as f64 / (t1 - t0) as f64)
}
