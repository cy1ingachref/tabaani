# Tabaani

Live public-transport tracking for Tunisia. Riders' phones are the sensors:
GPS pings go up, the server decides which line/trip they are on and where that
vehicle is now, and the map shows moving vehicles with honest
**Live / Estimated / Scheduled** badges.

Rail first (TGM pilot line), then bus. 2D MapLibre map; a 3D building/vehicle
mode is planned behind a toggle but **not** in the first slice.

- Plan & architecture: `docs/PLAN.md`
- Status: `docs/STATUS.md`
- Decisions: `docs/adr/`

## What works today

The rail matching core and its verification harness:

```
data/tab_seed.json          # one line (TGM Tunis Marine -> Marsa), 18 stops
        |  tools/gtfs-builder
        v
data/gtfs/build/<version>/  # GTFS + shape_dist_traveled
        |  tools/simulator
        v
data/traces/*.jsonl         # synthetic rider traces with ground truth
        |  tools/replay
        v
metrics vs thresholds.toml  # CI gate
```

```bash
make data      # seed -> GTFS
make traces    # GTFS -> simulated rider traces
make replay    # score traces through the matcher (fails below thresholds)
make test      # cargo test + fmt + clippy
```

Measured result of the current matcher on the synthetic set (see
`docs/STATUS.md` for the full table): 15/15 trips identified, median delay
error 0.1 s, off-route walker rejected.

## Layout

```
crates/transit-core/     geometry (linear referencing) + rail matcher
tools/gtfs-builder/      seed JSON -> versioned GTFS
tools/simulator/         GTFS trip -> fake phone traces (solves cold start)
tools/replay/            trace scorer + thresholds (the CI quality gate)
data/                    seed, built GTFS, traces
services/                api + matcher worker (next slice)
apps/mobile/             Flutter UI (later)
infra/                   compose, tiles, observability (later)
proto/                   ingest/live contracts (later)
```

## Design rules

- The timetable is a prior, not the truth. Delays are what we discover.
- Never show a scheduled position as if it were live.
- Publish vehicles, never riders. No persistent device ids.
- Small, verifiable increments; every change ships with tests and green CI.
