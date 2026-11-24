# 0001. Rail matcher is a dependency-free core crate

**Status:** Accepted

**Context.** The rail matcher must run in a hot path (per fix batch) and be unit
testable without a database or a geo stack. Pulling `geo`/`rstar`/PostGIS into
the maths makes tests slow and couples the algorithm to infrastructure.

**Decision.** Put linear referencing and the delay fit in
`crates/transit-core`, using only `f64` maths on a local east/north metre frame.
No external crates in that crate. The services layer can wrap it later.

**Consequences.**
- Fast, deterministic, trivially testable matching.
- We implement projection/interpolation ourselves (~200 lines) rather than
  calling PostGIS per fix; PostGIS remains the source of truth for storage.
- Distance maths is equirectangular over line-length extents (sub-metre error);
  long lines should be split or densified if that ever matters.

---

# 0002. Verify with a simulator + replay gate before any service exists

**Status:** Accepted

**Context.** The plan's biggest risk is "no riders = no data", which also means
"no way to know if the matcher works". A green build with no matcher behind it
would be a lie.

**Decision.** Build `tools/simulator` (traces with ground truth) and
`tools/replay` (scores traces against `thresholds.toml`, exits non-zero on
regression). CI runs them. Thresholds start loose and are tightened from
measured output.

**Consequences.**
- The matcher is provable today, not after the client exists.
- Traces become the durable regression set; real rides are added later.
- Any matcher change must keep the gate green, so accuracy cannot silently rot.

---

# 0003. 2D map first; 3D is a view layer behind a toggle

**Status:** Accepted

**Context.** A 3D building/vehicle mode is attractive but is a large scope add
(buildings with heights, procedural or glTF vehicles, quality manager) and does
nothing for tracking correctness.

**Decision.** Ship a 2D MapLibre map first. Keep vehicle state and geometry
client-side friendly so a 3D view can be added later without touching the
matcher. No terrain/DEM work in v1.

**Consequences.**
- Faster path to a useful product and to real rider data.
- 3D becomes an optional view with no impact on ingest/matching.
- If 3D is added, buildings come from the PMTiles building layer with a height
  heuristic where OSM tags are missing.

---

# 0004. Pilot line: TGM (Tunis Marine - La Marsa)

**Status:** Assumed (pending confirmation)

**Context.** Rail-first needs one line with good stop geometry. The seed has
the TGM (Transtu, metro mode), 18 stops, ~18.6 km.

**Decision.** Use the TGM as the pilot line and record the assumption.

**Consequences.**
- One corridor to validate end to end.
- Shapes are stop-to-stop polylines for now; real track centerlines arrive with
  the OSM step.
- Timings in the seed are synthetic placeholders, marked as such, and must be
  replaced before launch use.
