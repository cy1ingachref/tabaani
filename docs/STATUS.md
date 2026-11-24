# STATUS

Milestone tracker for `tabaani`. Updated at the end of each milestone.
See `docs/PLAN.md` for the architecture and `docs/adr/` for decisions.

## Phase 0 — Data, ground truth, core matcher: DONE (first slice)

### What was built

- `crates/transit-core`
  - `geometry.rs` — `LineProject`: linear referencing without a geo dependency.
    Projects a fix to (`s_m`, `d_lat_m`, `seg_idx`), interpolates a coordinate at
    `s`, clamps out-of-range input. Local east/north metre frame anchored at the
    shape so `cos(lat)` stays accurate over the whole line.
  - `rail.rs` — `RailMatcher`: per-trip delay fit
    `argmin_delta sum_i w_i (s_obs(t_i) - s_sched(t_i - delta))^2` with an
    asymmetric late>early delay prior, softmax posteriors, accept at
    posterior >= 0.8 and >= 2x runner-up, and a residual threshold that rejects
    poor fits outright.
  - 11 unit tests, including trip identification, delay recovery, honest
    ambiguity between two trips minutes apart, insufficient-evidence rejection,
    and off-window rejection.
- `tools/gtfs-builder` — seed JSON -> versioned GTFS. Writes
  `agency/stops/routes/trips/stop_times/shapes/calendar.txt` plus a
  `manifest.json`. Every `stop_time` carries `shape_dist_traveled`; shape point
  sequences are numbered.
- `tools/simulator` — reads the *built* GTFS back (so it cannot drift from the
  matcher) and writes one JSONL trace per trip with ground truth: real
  `shape_dist_traveled` metres, 4 m along-track noise, small lateral offset,
  deterministic wobble, plus one off-route walker control trace.
- `tools/replay` — projects each trace onto the shape, runs the real matcher,
  scores top-1 accuracy, false-positive rate, position error, delay error and
  calibration against `tools/replay/thresholds.toml`, exits non-zero on
  regression.

### How to run it

```bash
make data      # seed -> data/gtfs/build/<version>
make traces    # built GTFS -> data/traces/*.jsonl
make replay    # score traces; fails if a metric regresses
make test      # fmt + clippy -D warnings + cargo test
```

### What was measured

Last full pipeline run (see `make replay` output):

| Metric | Measured | Threshold |
|---|---|---|
| top1_accuracy | 1.000 (15/15) | >= 0.80 |
| false_positive | 0.000 (walker rejected) | <= 0.34 |
| position_median_m | 1.4 | <= 60 |
| position_p95_m | 1.4 | <= 200 |
| delay_median_s | 0.1 | <= 60 |
| calibration_ece | 0.000 (not yet sampled) | <= 0.30 |

`cargo test -p transit-core`: 11 passed. `cargo fmt --check` and
`cargo clippy --workspace --all-targets -- -D warnings`: clean.

### Honest gaps / next

- Synthetic timings in the seed; not real operator schedules.
- Simulated traces only; no real rides yet.
- Thresholds are starting values, to be tightened from measured output.
- No ingest/live services, no PostGIS/Redis, no client yet.
- `calibration_ece` is reported as 0 because replay does not yet collect
  confidence samples per decision; the ambiguity behaviour itself is unit-tested.

### Bug found and fixed during this phase (worth remembering)

The first fit compared observations to `s_sched(t + delta)` instead of
`s_sched(t - delta)`, which made a real 2 min delay come back as ~11 s. The
residual sign is now pinned by a unit test over the exact formula in PLAN §7.2.
