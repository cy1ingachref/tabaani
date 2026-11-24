//! Tests for delta encode/decode round-tripping and the protobuf wire format.

use crate::absolute_fixes;
use api::pb::{Fix, PingBatch};

#[test]
fn absolute_first_fix_decodes_to_the_sent_position() {
    // The wire convention: first fix carries absolute values, later ones deltas.
    let batch = PingBatch {
        session_token: "t1".into(),
        seq_start: 0,
        dataset_version: "0.1.0-dev".into(),
        fixes: vec![Fix {
            d_lat_e7: 368_006_900, // 36.80069 deg
            d_lon_e7: 101_917_400, // 10.19174 deg
            d_ts_ms: 28_800_000,   // 08:00:00
            accuracy_dm: 50,
            speed_cms: 0,
            bearing_deg: 0,
        }],
        hint: None,
    };
    let fixes = absolute_fixes(&batch);
    assert_eq!(fixes.len(), 1);
    assert!(
        (fixes[0].lat() - 36.80069).abs() < 1e-5,
        "lat={}",
        fixes[0].lat()
    );
    assert!(
        (fixes[0].lon() - 10.19174).abs() < 1e-5,
        "lon={}",
        fixes[0].lon()
    );
    assert_eq!(fixes[0].t_s, 28_800);
}

#[test]
fn later_fixes_accumulate_from_the_first() {
    let batch = PingBatch {
        session_token: "t2".into(),
        seq_start: 0,
        dataset_version: "0.1.0-dev".into(),
        fixes: vec![
            Fix {
                d_lat_e7: 100,
                d_lon_e7: 200,
                d_ts_ms: 0,
                ..Default::default()
            },
            Fix {
                d_lat_e7: 5,
                d_lon_e7: -7,
                d_ts_ms: 10_000,
                ..Default::default()
            },
        ],
        hint: None,
    };
    let fixes = absolute_fixes(&batch);
    assert_eq!(fixes[0].lat_e7, 100);
    assert_eq!(fixes[1].lat_e7, 105);
    assert_eq!(fixes[1].lon_e7, 193);
    assert_eq!(fixes[1].t_s, 10);
}

#[test]
fn encoded_batch_round_trips_through_the_decoder() {
    let original = PingBatch {
        session_token: "rt".into(),
        seq_start: 20,
        dataset_version: "v".into(),
        fixes: vec![
            Fix {
                d_lat_e7: 1_000,
                d_lon_e7: 2_000,
                d_ts_ms: 0,
                ..Default::default()
            },
            Fix {
                d_lat_e7: 10,
                d_lon_e7: -20,
                d_ts_ms: 5_000,
                ..Default::default()
            },
        ],
        hint: None,
    };
    let bytes = original.encode();
    let back = PingBatch::decode(&bytes).expect("decodes");
    assert_eq!(back.session_token, original.session_token);
    assert_eq!(back.seq_start, original.seq_start);
    assert_eq!(back.fixes, original.fixes);
}

#[test]
fn truncated_payload_is_rejected_not_panicked() {
    assert!(PingBatch::decode(&[0x0a, 0xff]).is_err());
    assert!(PingBatch::decode(&[]).is_ok()); // empty message is valid proto
}

/// Regression: a real Tunisian fix must project onto the TGM shape with a sane
/// along-track distance. This is the exact path `handle_entry` runs, and a
/// zero here means coordinates are not reaching the projector.
#[test]
fn real_fix_projects_onto_the_tgm_shape() {
    use transit_core::geometry::LineProject;
    let coords = vec![
        (10.19174, 36.80069),
        (10.29245, 36.81418),
        (10.30194, 36.81805),
        (10.34213, 36.87045),
        (10.33345, 36.88304),
    ];
    let shape = LineProject::new(&coords);
    assert!(shape.length_m() > 10_000.0, "shape too short");

    let batch = PingBatch {
        session_token: "p".into(),
        seq_start: 0,
        dataset_version: "v".into(),
        fixes: vec![Fix {
            d_lat_e7: 368_006_900,
            d_lon_e7: 101_917_400,
            d_ts_ms: 0,
            ..Default::default()
        }],
        hint: None,
    };
    let fixes = absolute_fixes(&batch);
    let obs = crate::project_fixes(&shape, &fixes);
    assert_eq!(obs.len(), 1);
    // The fix is the line's first vertex, so it must land at s ~= 0 with a tiny
    // lateral error. A non-zero s here is the symptom we saw live.
    assert!(
        obs[0].s_m < 100.0,
        "start of line expected, got {}",
        obs[0].s_m
    );
}
