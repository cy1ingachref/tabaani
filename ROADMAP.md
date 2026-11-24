# Tabaani roadmap goals.
#
# `make roadmap` runs `tools/roadmap` against this file and fails if any goal
# whose due phase is reached has an unchecked item. A goal is only checkable
# when its command actually works; `roadmap` refuses to mark an unchecked box as
# satisfied and refuses to pass a goal whose checks are all `[ ]`.
#
# Format:
#   ## GOAL <id>: <title>
#   status: done | in-progress | blocked | planned
#   phase: 0 | 1 | 2 | 3 | 4
#   check: shell command that must exit 0

## GOAL RO-1: Rail matcher core is verified by tests
status: done
phase: 0
check: cargo test -p transit-core
notes: dependency-free linear referencing + delay fit, 11 unit tests.

## GOAL RO-2: GTFS dataset builds from the seed with shape_dist_traveled
status: done
phase: 0
check: cargo run -q -p gtfs-builder -- data/tab_seed.json data/gtfs/build && test -s data/gtfs/build/0.1.0-dev/stop_times.txt
notes: agency/stops/routes/trips/stop_times/shapes/calendar + manifest.

## GOAL RO-3: Replay gate passes on generated traces
status: done
phase: 0
check: cargo run -q -p replay -- data/traces tools/replay/thresholds.toml
notes: scores top1/false-positive/position/delay/ece; exits non-zero on regression.

## GOAL RO-4: Ingest + matcher services are covered by integration tests
status: done
phase: 1
check: cargo test -p api -p matcher
notes: axum api (healthz/readyz/version, WS /v1/ingest, WS /v1/live,
      /v1/stops/{id}/arrivals) + Redis Streams matcher worker. Both binaries
      also verified live against a real redis container.

## GOAL RO-5: Whole-system check in one command
status: planned
phase: 1
check: cargo run -q -p roadmap
notes: depends on RO-4 (both services) and RO-6 (live e2e). Not due until the
      full pipeline is green.
notes: this file is the source of truth; make roadmap delegates here.

## GOAL RO-6: Simulated rider appears in the live feed within seconds
status: done
phase: 1
check: cargo run -q -p e2e
notes: needs `make services-up` first. Measured: trip tgm-004 recovered with
      delay exactly 120 s (the injected value), visible on /v1/live ~610 ms
      after the first batch. Target <3 s p95; measured 0.61 s.

## GOAL RO-7: Real rides replace synthetic timings in the replay set
status: planned
phase: 2
check: cargo run -q -p replay -- data/traces tools/replay/thresholds.toml
notes: blocked until real traces exist; the gate then measures reality.

## GOAL RO-8: Bus identification (stop sequence alignment)
status: planned
phase: 3
check: cargo test -p transit-core
notes: out of scope for this slice.
