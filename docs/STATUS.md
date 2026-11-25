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


---

## Phase 1 — Ingest + live pipeline: DONE (first slice)

### What was built

- `proto/tabaani/v1/transit.proto` — wire contract: `PingBatch`/`Fix`/`Hint`
  upstream, `VehicleState`/`LiveFrame`/`Confidence` downstream.
- `services/api` (axum)
  - `GET /healthz`, `/readyz` (probes Postgres + Redis; reports **degraded**
    honestly when a dependency is absent), `/version`.
  - `WS /v1/ingest` — binary protobuf batches; idempotent per
    `(session_token, seq_end)` so retries never double-count; 64 KiB batch cap;
    malformed payloads answered, not crashed on.
  - `WS /v1/live` — snapshot on connect, then delta frames from `live:all`.
  - `GET /v1/stops/{id}/arrivals` — vehicles currently known to Redis.
- `services/matcher` (worker)
  - Redis Streams consumer group; re-asserts the group at id 0 each tick so a
    reset stream cannot silently orphan it.
  - Loads the GTFS network at boot (shape from stop geometry, trip profiles from
    `shape_dist_traveled`).
  - Delta-decodes fixes (first fix absolute, later ones deltas), projects onto
    the route, accumulates per-session observations in Redis, runs
    `transit-core`'s rail matcher, publishes live state + pub/sub delta.
- `services/api/src/state.rs` — one shared `StoredVehicle` schema, used by both
  services so they cannot drift.
- `tools/e2e` — real WebSocket rider: ingest -> matcher -> live feed, measuring
  ping->visible latency.
- `tools/roadmap` + `ROADMAP.md` — goal file; every due goal has a `check:`
  command. `make roadmap` runs them all and fails if any does not pass.

### How to run it

```bash
make services-up     # redis container + api + matcher
make e2e             # live end-to-end gate
make roadmap         # check every due ROADMAP goal
```

### What was measured (live, against a real redis container)

| Check | Measured | Target |
|---|---|---|
| trip identified | tgm-004 | the simulated rider's trip |
| fitted delay | 120.0 s | injected exactly 120 s |
| ping -> visible on /v1/live | ~0.61 s | < 3 s p95 |
| api /readyz with deps down | 503 degraded | honest degradation |

Replay gate unchanged: top1_accuracy 1.000, false_positive 0.000,
position_median 1.4 m, delay_median 0.1 s. Tests: 21 passing across
transit-core (11), matcher (5), api (5).

### Bugs the live gate caught (and fixed)

- XREADGROUP reply shape mis-decoded by redis-rs -> raw `Value` parser.
- Consumer group anchored at `$`: invisible after a stream reset -> re-assert at 0.
- Binary protobuf read back as lossy UTF-8 -> refetch raw bytes via XRANGE.
- Implicit delta origin -> first fix carries absolute coordinates.
- e2e subscribed only after publishing -> subscribe before sending.

### Honest gaps / next

- Postgres persistence is not wired; `/readyz` reports it degraded by design.
- Confidence is `estimated`; multi-rider fusion (LIVE) arrives with Phase 3.
- Timings in the seed are still synthetic placeholders.
- No client yet; no bus matching.


---

## Phase 1b — Postgres + PostGIS persistence: DONE

### What was built

- `services/api/migrations/0001_network.sql` — full plan §9 schema: postgis +
  pg_trgm extensions, `dataset_version`, `stops`/`routes`/`shapes`/`trips`/
  `stop_times` with GiST spatial indexes and a trigram stop-name index, plus the
  operational tables (`sessions`, `segment_stats`, `trip_delays`, `labels`).
- `tools/db` — migrate / load-gtfs / status. Migrations run one per transaction
  and are recorded in `schema_migrations`; loading is idempotent (clears the
  previous network then inserts inside one transaction).
  - Uses `tokio-postgres`, not `sqlx`: this is a one-shot CLI, so no compile-time
    query macros, and the workspace still builds with no database present.
  - **Header-aware CSV parsing**: every loader resolves columns by name from the
    file's own header. Hard-coded indices silently mis-load when GTFS producers
    reorder columns — which is exactly what happened (see below).
- `api/src/lib.rs` now probes Postgres for real; `/readyz` no longer reports a
  fabricated degraded state.

### What was measured (real PostGIS 3.4 on PostgreSQL 16)

```
dataset_version: 0.1.0-dev
  stops 18   routes 1   shapes 1   trips 15   stop_times 270   postgis true
shape length      18576 m   (builder computed 18576 m — exact match)
termini apart     15598 m straight-line (PostGIS geography)
GET /readyz       {"status":"ready","checks":{"postgres":{"ok":true},"redis":{"ok":true}}}
```

### Bugs found by loading into Postgres and fixed

1. **gtfs-builder wrote trips.txt values in the wrong column order** while the
   header claimed `route_id,service_id,trip_id,...`. Nothing caught it until the
   FK constraint failed. Fixed at the source (values now match the header).
2. **Hard-coded CSV column indices in the loader** would have mis-loaded any
   producer that orders columns differently. Replaced with header-driven access.
3. **Rust string continuations** (`\` + newline) swallowed SQL spaces, producing
   `UPDATESET`. Rewritten as a single-space continuation.

### Local port note

This host already has a native Windows `postgres.exe` on 5432, so the project's
PostGIS container uses **55432**. `DATABASE_URL` defaults to
`postgres://tabaani:***@127.0.0.1:55432/tabaani`. CI maps 55432 explicitly.

### Honest gaps / next

- The matcher still loads its network from GTFS files at boot, not from Postgres;
  reading the live network from PostGIS is the next step.
- Operational tables exist but are not written yet (no ping persistence).
- Seed timings remain synthetic.


---

## RO-5 — Whole-system check in one command: DONE

`make verify` from a clean checkout now brings up PostGIS + Redis, builds and
tests everything, runs the replay gate, migrates + loads + checks the database,
starts api + matcher, runs the live e2e, and leaves the stack up for inspection.
`make roadmap` runs all goal checks and now passes 9/9 (7 done, 2 planned-skip).

### Bugs found while making this a single command and fixed

1. **roadmap tool hung on backgrounded services** — child processes inherited
   the tool's stdout/stderr pipes; `cmd /C` waited forever for them to close.
   The tool now spawns checks with null stdio (it reports PASS/FAIL itself).
2. **stale api/matcher binaries blocked `cargo test`** — `make services-down`
   now stops only processes whose executable lives in this repo's
   `target/debug`, via a generated PowerShell helper (no sweep-kill).
3. **Windows `$!` gave MSYS shell PIDs, not Windows PIDs** — replaced with
   path-scoped process matching.
4. **Postgres readiness matched the bootstrap server's log line** — now waits
   for the final `[1] LOG: database system is ready...` line, and `db` retries
   its full handshake (connect + SET) 10 times.
5. **services-up hard-coded a Redis container** — now reuses an already
   reachable Redis (as on CI), so it works both locally and on GitHub runners.

### Measured

  make verify        PASS (e2e delay=120 s, visible in ~615 ms)
  make roadmap       9 goals: all due goals pass
  cargo test          21 passed / 0 failed
  fmt / clippy        clean
  actionlint          clean
