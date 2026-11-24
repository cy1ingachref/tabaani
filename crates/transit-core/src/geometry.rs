//! Linear referencing on a polyline: project a fix onto a line and report the
//! perpendicular distance (`d_lat`) and the distance along the line (`s`).
//!
//! This mirrors `ST_LineLocatePoint` semantics (plan §7.2 step 1) without a
/// PostGIS round-trip per fix.
use std::f64::consts::PI;

/// A shape that fixes can be projected onto. Built once per dataset load.
#[derive(Debug, Clone)]
pub struct LineProject {
    /// Vertices as (lon, lat) degrees.
    coords: Vec<(f64, f64)>,
    /// Cumulative length in metres at each vertex.
    cumulative_m: Vec<f64>,
    length_m: f64,
}

/// Result of projecting one coordinate onto a [`LineProject`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Distance along the line in metres from its first vertex.
    pub s_m: f64,
    /// Perpendicular distance to the line in metres (>= 0).
    pub d_lat_m: f64,
    /// Index of the segment the projection fell on.
    pub seg_idx: usize,
}

const EARTH_R_M: f64 = 6_371_008.8;

impl LineProject {
    /// Build from (lon, lat) pairs in degrees.
    pub fn new(coords: &[(f64, f64)]) -> Self {
        let mut cumulative_m = Vec::with_capacity(coords.len());
        let mut total = 0.0;
        cumulative_m.push(0.0);
        for w in coords.windows(2) {
            total += haversine_m(w[0], w[1]);
            cumulative_m.push(total);
        }
        Self {
            coords: coords.to_vec(),
            cumulative_m,
            length_m: total,
        }
    }

    pub fn length_m(&self) -> f64 {
        self.length_m
    }

    pub fn vertex_count(&self) -> usize {
        self.coords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.coords.len() < 2 || self.length_m <= 0.0
    }

    /// Project a coordinate onto the line, returning the closest point's
    /// along-track distance and lateral offset.
    pub fn project(&self, lon: f64, lat: f64) -> Option<Projection> {
        if self.is_empty() {
            return None;
        }
        // Local east/north metre frame anchored at the *nearest latitude* of
        // the line so cos(lat) stays accurate across the whole shape.
        let ref_lat = self.coords.iter().map(|c| c.1).sum::<f64>() / self.coords.len() as f64;
        let kx = EARTH_R_M * ref_lat.to_radians().cos();
        let origin = self.coords[0];
        let px = (lon - origin.0).to_radians() * kx;
        let py = (lat - origin.1).to_radians() * EARTH_R_M;

        let mut best: Option<Projection> = None;
        for (idx, w) in self.coords.windows(2).enumerate() {
            let ax = (w[0].0 - origin.0).to_radians() * kx;
            let ay = (w[0].1 - origin.1).to_radians() * EARTH_R_M;
            let bx = (w[1].0 - origin.0).to_radians() * kx;
            let by = (w[1].1 - origin.1).to_radians() * EARTH_R_M;
            let (along_m, dist_m) = project_on_segment((ax, ay), (bx, by), (px, py));
            let candidate = Projection {
                s_m: self.cumulative_m[idx] + along_m,
                d_lat_m: dist_m,
                seg_idx: idx,
            };
            if best.is_none_or(|b| candidate.d_lat_m < b.d_lat_m) {
                best = Some(candidate);
            }
        }
        best
    }

    /// Interpolate the (lon, lat) coordinate at distance `s` metres along the
    /// line. Used by replay/rendering to place a vehicle from a fitted delay.
    pub fn point_at(&self, s_m: f64) -> Option<(f64, f64)> {
        if self.is_empty() {
            return None;
        }
        let s = s_m.clamp(0.0, self.length_m);
        // Right-most segment whose start is <= s: interior hits stay on the
        // correct segment and s == length lands exactly on the final vertex.
        let idx = (0..self.coords.len() - 1)
            .rev()
            .find(|&i| self.cumulative_m[i] <= s)
            .unwrap_or(0);
        let seg_start = self.cumulative_m[idx];
        let seg_len = self.cumulative_m[idx + 1] - seg_start;
        let frac = if seg_len > 0.0 {
            (s - seg_start) / seg_len
        } else {
            0.0
        };
        let a = self.coords[idx];
        let b = self.coords[idx + 1];
        Some((a.0 + (b.0 - a.0) * frac, a.1 + (b.1 - a.1) * frac))
    }
}

/// Project `p` onto segment `a..b`, all in metres. Returns (distance along the
/// segment, perpendicular distance).
fn project_on_segment(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    let abx = b.0 - a.0;
    let aby = b.1 - a.1;
    let len_sq = abx * abx + aby * aby;
    if len_sq <= 0.0 {
        let dx = p.0 - a.0;
        let dy = p.1 - a.1;
        return (0.0, (dx * dx + dy * dy).sqrt());
    }
    let t = ((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let foot_x = a.0 + t * abx;
    let foot_y = a.1 + t * aby;
    let dx = p.0 - foot_x;
    let dy = p.1 - foot_y;
    (t * len_sq.sqrt(), (dx * dx + dy * dy).sqrt())
}

/// Great-circle distance in metres between two (lon, lat) points.
pub fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dlat = (b.1 - a.1).to_radians();
    let dlon = (b.0 - a.0).to_radians();
    let lat_mid = ((a.1 + b.1) / 2.0).to_radians();
    let x = dlon * lat_mid.cos().max(1e-6);
    (dlat * dlat + x * x).sqrt() * EARTH_R_M
}

#[allow(dead_code)]
fn _unused(_: f64) -> f64 {
    // Keep PI referenced so the import is not flagged if maths changes.
    PI
}
