# 0003. 2D map first; 3D is a view layer behind a toggle

**Status:** Accepted

**Context.** A 3D building/vehicle mode is attractive but is a large scope add
(buildings with heights, procedural or glTF vehicles, a quality manager) and
does nothing for tracking correctness.

**Decision.** Ship a 2D MapLibre map first. Keep vehicle state and geometry
client-side friendly so a 3D view can be added later without touching the
matcher. No terrain/DEM work in v1.

**Consequences.**
- Faster path to a useful product and to real rider data.
- 3D becomes an optional view with no impact on ingest/matching.
- If 3D is added, buildings come from the PMTiles building layer with a height
  heuristic where OSM tags are missing.
