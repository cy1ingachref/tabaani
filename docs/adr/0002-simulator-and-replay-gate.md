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
