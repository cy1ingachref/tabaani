//! Tests for linear referencing and the rail delay fit.

use crate::geometry::{haversine_m, LineProject};
use crate::rail::{ObsFix, RailDecision, RailMatcher, TripTiming};

/// TGM pilot line: (lon, lat) from the vendored seed, Tunis Marine -> Marsa.
fn tgm_coords() -> Vec<(f64, f64)> {
    vec![
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
    ]
}

fn tgm_shape() -> LineProject {
    LineProject::new(&tgm_coords())
}

#[test]
fn projects_a_point_beside_the_line() {
    let shape = tgm_shape();
    // Offset ~40 m purely in longitude from vertex 2, at the same latitude.
    let (lon, lat) = (10.30194, 36.81805);
    let offset = 40.0 / (111_320.0 * 36.8_f64.to_radians().cos());
    let p = shape.project(lon + offset, lat).expect("projects");
    assert!(
        p.d_lat_m > 20.0 && p.d_lat_m < 60.0,
        "d={} m (want ~40)",
        p.d_lat_m
    );
    // The closest point is on the segment that contains vertex 2's approach;
    // `s` must therefore be near vertex 2 (not at the line start).
    let s_v1 = shape.project(10.29245, 36.81418).unwrap().s_m;
    assert!(p.s_m > s_v1, "s={} should be past v2 ({})", p.s_m, s_v1);
    assert!(p.seg_idx >= 1, "seg={}", p.seg_idx);
}

#[test]
fn vertices_are_ordered_and_segment_indices_advance() {
    let shape = tgm_shape();
    let mut prev = -1.0;
    for &(lon, lat) in &tgm_coords() {
        let p = shape.project(lon, lat).expect("vertex projects");
        assert!(p.d_lat_m < 1.0, "vertex d={}", p.d_lat_m);
        assert!(p.s_m >= prev, "s went backwards: {prev} -> {}", p.s_m);
        prev = p.s_m;
    }
}

#[test]
fn projects_vertices_with_zero_lateral_error() {
    let shape = tgm_shape();
    let mut prev = -1.0;
    for &(lon, lat) in &tgm_coords() {
        let p = shape.project(lon, lat).expect("vertex projects");
        assert!(p.d_lat_m < 1.0, "vertex d={}", p.d_lat_m);
        assert!(p.s_m >= prev, "s went backwards: {prev} -> {}", p.s_m);
        prev = p.s_m;
    }
}

#[test]
fn rejects_a_far_away_point() {
    let shape = tgm_shape();
    let p = shape
        .project(10.6, 36.9)
        .expect("still projects to nearest");
    assert!(p.d_lat_m > 10_000.0, "d={}", p.d_lat_m);
}

#[test]
fn interpolates_and_clamps() {
    let shape = tgm_shape();
    let mid = shape.point_at(shape.length_m() / 2.0).unwrap();
    assert!(mid.0 > 10.19 && mid.0 < 10.34, "lon={}", mid.0);
    // Clamping: out-of-range s lands on the endpoints.
    assert_eq!(shape.point_at(-5.0), shape.point_at(0.0));
    assert_eq!(
        shape.point_at(1e9),
        shape.point_at(shape.length_m()),
        "s beyond length must clamp to the final vertex"
    );
}

#[test]
fn single_point_and_empty_shapes_are_empty() {
    assert!(LineProject::new(&[]).is_empty());
    assert!(LineProject::new(&[(10.19, 36.8)]).is_empty());
    assert_eq!(LineProject::new(&[]).project(10.0, 36.0), None);
}

#[test]
fn haversine_matches_expected_scale() {
    // One degree of latitude is ~111 km.
    let d = haversine_m((10.0, 36.0), (10.0, 37.0));
    assert!((d - 111_000.0).abs() < 2_000.0, "d={}", d);
    // A degree of longitude at 36.8 N is ~89 km.
    let e = haversine_m((10.0, 36.8), (11.0, 36.8));
    assert!((e - 89_000.0).abs() < 2_000.0, "e={}", e);
}

// --- rail matcher -------------------------------------------------------------

/// A synthetic trip over the whole line: depart `start_s`, average `speed_mps`
/// between stops, dwell `dwell_s` at every stop except the last.
fn synthetic_trip(
    trip_id: &str,
    start_s: i64,
    length_m: f64,
    n_stops: usize,
    speed_mps: f64,
    dwell_s: i64,
) -> TripTiming {
    let mut profile = Vec::new();
    for k in 0..n_stops {
        let frac = k as f64 / (n_stops - 1).max(1) as f64;
        let s = length_m * frac;
        let t = start_s + (length_m * frac / speed_mps) as i64;
        profile.push((t, s));
        if k + 1 < n_stops {
            profile.push((t + dwell_s, s));
        }
    }
    TripTiming {
        trip_id: trip_id.into(),
        start_s,
        profile,
    }
}

/// Observed fixes for a rider riding `trip` delayed by `delay_s`.
///
/// The observation is the *true* along-track position, perturbed by a small
/// deterministic wobble so the fit is not trivially exact.
fn ride_obs(shape: &LineProject, trip: &TripTiming, delay_s: i64, noise_m: f64) -> Vec<ObsFix> {
    let start = trip.profile[0].0 + 20;
    let end = trip.profile.last().unwrap().0 - 20;
    let mut obs = Vec::new();
    let mut t = start;
    let mut i = 0i64;
    while t <= end {
        // Where the vehicle actually was at time t (the schedule shifted by
        // the true delay). This is the ground truth we want recovered.
        let true_s = trip.s_at(t - delay_s).unwrap_or(0.0);
        let wobble = (((i * 2_654_435_761) % 1000) as f64 / 1000.0 * 2.0 - 1.0) * noise_m;
        obs.push(ObsFix {
            t_s: t,
            s_m: (true_s + wobble).clamp(0.0, shape.length_m()),
            accuracy_m: 5.0,
        });
        t += 10;
        i += 1;
    }
    obs
}

#[test]
fn identifies_the_right_trip_and_delay() {
    let shape = tgm_shape();
    let len = shape.length_m();
    let trips = vec![
        synthetic_trip("T1", 5 * 3600, len, 6, 9.0, 20),
        synthetic_trip("T2", 6 * 3600, len, 6, 9.0, 20),
        synthetic_trip("T3", 7 * 3600, len, 6, 9.0, 20),
    ];
    let obs = ride_obs(&shape, &trips[1], 120, 3.0); // 2 min late
    let matcher = RailMatcher::new(&shape);
    match matcher.decide(&obs, 6 * 3600 + 300, &trips) {
        RailDecision::Identified {
            trip_id,
            delay_s,
            confidence,
        } => {
            assert_eq!(trip_id, "T2");
            assert!((delay_s - 120.0).abs() < 20.0, "delay={}", delay_s);
            assert!(confidence > 0.8, "conf={}", confidence);
        }
        other => panic!("expected Identified, got {:?}", other),
    }
}

#[test]
fn ambiguous_when_two_trips_are_a_few_minutes_apart() {
    let shape = tgm_shape();
    let len = shape.length_m();
    // Two trips 4 minutes apart. A trace at the midpoint fits both almost
    // equally well, which is exactly the honest-ambiguity case (plan §7.2).
    let trips = vec![
        synthetic_trip("A", 6 * 3600, len, 6, 9.0, 20),
        synthetic_trip("B", 6 * 3600 + 240, len, 6, 9.0, 20),
    ];
    // Ride B but sample only its first half, where A and B overlap closely.
    let obs = ride_obs(&shape, &trips[1], 0, 3.0);
    let matcher = RailMatcher::new(&shape);
    match matcher.decide(&obs, 6 * 3600 + 300, &trips) {
        RailDecision::Ambiguous { candidates } => {
            assert!(candidates.len() >= 2, "got {:?}", candidates);
            // The two candidates should be close in posterior.
            let a = candidates[0].posterior;
            let b = candidates.get(1).map(|c| c.posterior).unwrap_or(0.0);
            assert!(a < 0.95, "should not be confident: {}", a);
            assert!(b > 0.0, "runner-up missing");
        }
        other => panic!("expected Ambiguous, got {:?}", other),
    }
}

#[test]
fn too_few_fixes_is_insufficient() {
    let shape = tgm_shape();
    let trips = vec![synthetic_trip("T", 6 * 3600, shape.length_m(), 6, 9.0, 20)];
    let obs = vec![
        ObsFix {
            t_s: 6 * 3600,
            s_m: 0.0,
            accuracy_m: 5.0,
        },
        ObsFix {
            t_s: 6 * 3600 + 10,
            s_m: 10.0,
            accuracy_m: 5.0,
        },
    ];
    let matcher = RailMatcher::new(&shape);
    match matcher.decide(&obs, 6 * 3600, &trips) {
        RailDecision::Insufficient { .. } => {}
        other => panic!("expected Insufficient, got {:?}", other),
    }
}

#[test]
fn a_trace_on_a_different_day_is_not_explained_by_any_trip() {
    let shape = tgm_shape();
    let len = shape.length_m();
    let trips = vec![synthetic_trip("T", 6 * 3600, len, 6, 9.0, 20)];
    // Observations far outside any plausible window: the window gate drops the
    // trip and we report honestly.
    let obs: Vec<ObsFix> = (0..10)
        .map(|k| ObsFix {
            t_s: 23 * 3600 + k * 10,
            s_m: 500.0 + k as f64 * 10.0,
            accuracy_m: 5.0,
        })
        .collect();
    let matcher = RailMatcher::new(&shape).with_window_s(60);
    match matcher.decide(&obs, 23 * 3600, &trips) {
        RailDecision::Insufficient { .. } => {}
        other => panic!("expected Insufficient, got {:?}", other),
    }
}
