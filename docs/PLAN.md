# Tabaani: Live Transit Tracking App — Plan & Architecture

**Project:** **Tabaani**, a standalone live public-transport tracking app for Tunisia
**Scope:** Track buses, trains, metro and trams in Tunisia in real time, using riders' phones as the sensors.
**Priority order:** rail first (train, metro léger, tram/TGM), then buses.
**Document status:** v3, planning + first slice implemented. Numbers marked *(target)* are design goals to validate with real data, not guarantees.

---

## 0. TL;DR

- Riders' phones send GPS pings. The server decides **(a)** are they on public transport, **(b)** which line/trip, and **(c)** where the vehicle is now. Many riders on one vehicle are fused into one position.
- **Rail** is the easy case: snap to the track geometry, measure position along the track, compare with timetables to find the exact trip and its delay.
- **Bus** is harder: roads are shared with cars and lines overlap. Identification uses the *sequence of stops passed*, speed, direction and time, scored as probabilities and refined as more evidence arrives.
- **The timetable is a prior, not the truth.** Delays are exactly what the app should discover. Matching must tolerate lateness, and ambiguity must be shown honestly ("Line 20 or 21") until the evidence separates them.
- **Stack (this slice):** Rust core crate with no heavy dependencies, GTFS built from a seed, a simulator that produces traces with ground truth, and a replay gate that scores the matcher.
- **Later slices:** axum `api` + `matcher` worker, PostgreSQL/PostGIS + Redis, Flutter UI with a 2D MapLibre map (3D mode behind a toggle).
- **Biggest risks:** cold start (no riders = no live data), battery drain, and location privacy. Each has a concrete mitigation below.

---

## 1. Product definition

### 1.1 Goals
1. Show **live positions** of buses, trains, metro and trams on a map.
2. Show **real delays** and **ETAs** per stop.
3. Identify the line/trip automatically when a rider boards, with a one-tap confirm/correct option.
4. Be **fast**, **reliable** (bad networks, tunnels, restarts) and **accurate**.
5. 2D map first; a 3D building/vehicle mode is a later view-layer addition and does not change tracking.

### 1.2 Non-goals (v1)
- Official operator integrations (pluggable later).
- Ticketing/payments.
- iOS at launch (Android first).
- Crowding prediction, carpooling.
- A full journey planner in v1 (Phase 4).

### 1.3 User stories
| Who | Story |
|---|---|
| Rider (contributor) | I open nothing; the app notices I'm on the metro and quietly contributes. I can confirm "Yes, Ligne 4 → Bardo". |
| Waiting passenger | I see the next vehicles at my stop with a "live" or "scheduled" badge and a delay. |
| Operator/researcher | I can export anonymized delay stats as GTFS-Realtime / CSV (later). |

---

## 2. Core idea and honest constraints

### 2.1 Why it works
Phones give position (GPS ~5–10 m in open sky), speed, heading and an activity hint. Public transport follows fixed geometry and roughly fixed schedules, so a short trace usually has very few plausible explanations.

### 2.2 Constraints
| Constraint | Consequence |
|---|---|
| **No riders = no data** | Cold start. Show *scheduled* positions (clearly labelled) until a rider is detected. Seed with your own team, a driver mode, and partner fleets. |
| **Timetables unreliable/missing** (esp. buses) | Schedules are a prior; learn real headways from data; fall back to headway matching. |
| **Overlapping bus lines** | Ambiguity until lines diverge. Keep a candidate set and **retroactively relabel** (fixed-lag smoothing). |
| **Cars/taxis look like buses** | Require stop-passage patterns, dwell, activity, consensus, user confirmation. |
| **Walking beside the track** | Require sustained speed and along-track progression, plus a residual threshold. |
| **GPS degrades** (tunnels, canyons) | Weight fixes by reported accuracy; dead-reckon; never snap blindly. |
| **Battery** | Adaptive sampling, native tracker, on-device gating. |
| **Location is sensitive** | Privacy-by-design, short retention, no persistent trails. |

### 2.3 Hybrid identification
Pure automatic inference is the goal, but make it **assisted**: below a confidence threshold the app asks once ("Are you on Line 20 or 21?"). Every answer is a labelled example that improves the matcher.

---

## 3. Architecture (target, built incrementally)

```
Phone: native tracker -> on-device gate -> SQLite buffer -> batched protobuf pings
Server: ingest API -> Redis Streams -> matcher workers -> Redis live state -> WS gateway -> clients
Data: PostGIS (GTFS + geometry) ; Postgres (history/aggregates)
```

This slice implements the **matcher + its data + its verification**. The
ingest/live pipeline lands next, then the client.

---

## 4. Technology choices

| Layer | Choice | Why |
|---|---|---|
| Core maths | **Rust crate, dependency-free** | Fast, testable, no geo stack in the hot path |
| Data format | **GTFS** (built by our tool) | Standard tooling, interoperable |
| Verification | **Simulator + replay gate** | Proves the matcher without real riders |
| Backend (next) | Rust `axum` + `tokio` + `sqlx` + `prost` | Low latency, one binary |
| Geo (next) | PostgreSQL 16 + PostGIS, Redis 7 | Linear referencing, geo index, streams |
| Client (next) | Flutter + MapLibre (2D first) | One codebase; offline-capable vector map |
| Tiles (next) | Self-hosted PMTiles | No dependence on public tile servers |
| Protocol (next) | Protobuf over WebSocket | Compact, typed |

**Prototype tip:** tune the algorithm against recorded/simulated traces first,
then wire it into services. The traces become the regression set.

---

## 5. Data layer

### 5.1 Static data needed
1. Stops (id, name, lat/lon).
2. Routes/lines (id, short name, mode, operator, direction).
3. Shapes (the polyline each trip follows; exact track centerline for rail).
4. Trips and stop_times; **frequencies** where no timetable exists.
5. Calendar (weekday/weekend/holiday/Ramadan variants).

### 5.2 Sources
- **Seed:** hand-built stops/routes for pilot lines (OSM + operator timetables).
- **OpenStreetMap** relations for rail/bus routes and shapes.
- Operator timetables (check what each publishes and under what terms).
- **Your own field data:** ride each line with a logger app (ground truth).

### 5.3 Quality pipeline
- Validate GTFS in CI.
- Reject/quarantine stops without coordinates.
- Snap every stop to its shape and store `shape_dist_traveled`. **Everything runs on this distance axis.**
- Version the dataset; record the version used per match.

> The pilot seed in `data/tab_seed.json` carries synthetic timings clearly marked
> as such. Replace with real operator times before any launch use.

---

## 6. Mobile client (later)

- Native Kotlin tracker (foreground service) + Flutter UI via platform channel.
- State machine: IDLE → CANDIDATE → ON_TRANSIT → MATCHED → ENDING.
- On-device gate: download a buffered transit footprint; never upload off-network fixes.
- Adaptive sampling; batch 5–10 s of fixes; delta-encoded protobuf.
- Offline-first SQLite buffer with sequence numbers; server accepts late/out-of-order batches.
- Markers interpolated locally between updates; badges **Live (n riders) / Estimated / Scheduled**.

---

## 7. Detection and matching engine

### 7.1 Pipeline
```
raw fixes -> clean -> mode gate -> map-match -> candidate trips -> score -> decide -> fuse -> publish
```
Clean: drop fixes worse than ~50 m accuracy, impossible jumps, duplicate timestamps; smooth.
Mode gate: features over a 30–60 s window (speed, acceleration, stop-go, heading variance, activity label, distance to rail vs road).

### 7.2 Rail (implemented in `transit-core`)

**Step 1 — Snap to track.** Project each fix onto the nearest rail line:
- `d_lat` = perpendicular distance (m), `s` = distance along (m).
Accept "on rail" when `d_lat` < ~25 m for N consecutive fixes **and** `s` progresses monotonically at a plausible speed.

**Step 2 — Which trip.** For each scheduled trip within the window, build `s_sched(t)` and estimate the delay that best aligns observations:

```
delta*(trip) = argmin_delta  sum_i  w_i ( s_obs(t_i) - s_sched(t_i - delta) )^2
```

Keep plausible `delta` (late more likely than early); score with fit residual + asymmetric delay prior.

**Step 3 — Decide.** Posterior over trips; accept when top posterior >= 0.8 and >= 2× runner-up, else show top-2 as "probably". Direction comes free from the sign of `ds/dt`.

**Implemented and verified:** the core above lives in
`crates/transit-core/src/rail.rs`; `tools/replay` scores it against traces.
A residual threshold rejects poor fits outright (this is what stops a walker
beside the line being accepted as a weak match).

**Known ambiguity (be honest about it):** if the real delay is about half the
headway, two trips explain the data equally. Resolve with the delay prior,
dwell at stations passed, other riders, and the confirm button. TGM headways
are long enough that this is rare, but it is handled, not ignored.

**Rail bonus:** the fitted `delta*` *is* the live delay. Identification and
delay estimation are the same computation.

### 7.3 Bus (later)
Stop-passage events (not only stops served), sequence alignment against
candidate routes, top-K candidates on shared corridors, back-labeling, car-vs-bus
evidence, multi-rider fusion with a 1-D Kalman on the route axis.

### 7.4 Failure modes
| Failure | Response |
|---|---|
| GPS gap | Dead-reckon up to ~60 s, widen uncertainty, mark "estimated" |
| Two candidates tied | Show both; ask; wait for divergence |
| Rider gets off | End session; stop publishing |
| Spoofed/wild data | Outlier rejection, trust score, consensus |
| Schedule changed/cancelled | Flag only with strong evidence |

---

## 8. Privacy, anti-abuse, legal

- No account needed to contribute; random **rotating per-trip token**, never a persistent device id.
- On-device gate means home/work locations are never collected.
- Retention: raw pings deleted after a short window; keep anonymous aggregates.
- **Publish vehicles, never riders.**
- Anti-abuse: mock-location flags, impossible kinematics, trust score, signed ingest tokens, k-rider consensus before a single phone can move a published vehicle.
- Legal: Tunisia's personal-data framework (Organic Law 2004-63, INPDP). Location is personal data; check obligations before launch. Not legal advice.

---

## 9. Roadmap

### Phase 0 — Data, ground truth, core matcher (this slice)
- Seed + GTFS builder + simulator + replay gate. **Done** (see `docs/STATUS.md`).
- Exit: replay passes thresholds; wrong-line/off-route traces rejected.

### Phase 1 — Ingest + live pipeline
- axum `api` + `matcher` worker, Redis Streams consumer groups, Redis live state,
  `/v1/live` WS deltas, idempotent ingest, late-batch handling.
- Exit: a simulated vehicle appears on a test client within seconds; chaos test passes.

### Phase 2 — Client MVP (2D)
- Native tracker + Flutter UI + MapLibre, arrivals, confirm/correct, badges,
  offline stops/shapes. 2D map only; 3D is a later view layer.
- Exit: a real phone ride shows the vehicle on a second phone.

### Phase 3 — Bus
- Stop-passage detection, sequence alignment, top-K, back-labeling, fusion, ETA v1.

### Phase 4 — Hardening, launch, growth
- Battery pass, OEM-kill handling, observability, backups/restore drill,
  privacy policy, closed beta, then iOS and (optionally) 3D.

---

## 10. Risk register (top)
| Risk | Mitigation |
|---|---|
| Not enough riders | Rail-first, driver mode, partner fleets, gamification, scheduled fallback |
| Android background killing | Foreground service, OEM guidance, restart logic |
| Battery complaints | Gate, adaptive sampling, batching, measure early |
| Bus line ambiguity | Top-K, back-labeling, confirm button, honest labels |
| Bad/missing timetables | Frequency priors, learn from data |
| Car mistaken for bus | Stop-pattern evidence, consensus, trust score |
| Privacy/legal | Gate, short retention, no ids, legal check |

---

## 11. Immediate next steps
1. Wire the ingest protocol (`proto/`) and the `api`/`matcher` services.
2. Add Postgres/PostGIS persistence + Redis live state.
3. Record real rides on the pilot line and add them to the replay set.
4. Replace synthetic seed timings with operator data.
