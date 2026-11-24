//! Rail identification and delay fitting (plan §7.2).
//!
//! The timetable is a *prior*, not the truth: we estimate the delay `delta` that
//! best aligns the observed track with each scheduled trip, then score trips by
//! fit residual plus an asymmetric delay prior (late is more likely than early).

use crate::geometry::LineProject;

/// A scheduled trip reduced to what the matcher needs.
#[derive(Debug, Clone)]
pub struct TripTiming {
    pub trip_id: String,
    /// Seconds after midnight of the trip's first departure.
    pub start_s: i64,
    /// (seconds after midnight, distance along the line in metres) per stop.
    /// Must be sorted by time; dwell is encoded as repeated points at a stop.
    pub profile: Vec<(i64, f64)>,
}

impl TripTiming {
    /// Build from explicit (time_s, s_m) samples. Useful for tests and for
    /// hand-built timetables.
    pub fn from_samples(trip_id: &str, start_s: i64, samples: Vec<(i64, f64)>) -> Self {
        Self {
            trip_id: trip_id.into(),
            start_s,
            profile: samples,
        }
    }

    /// Scheduled position along the line at `t_s` seconds after midnight.
    /// Piecewise linear between stops, clamped at both ends (dwell at the
    /// terminus before departure and after arrival).
    pub fn s_at(&self, t_s: i64) -> Option<f64> {
        let first = self.profile.first()?;
        let last = self.profile.last()?;
        if t_s < first.0 || t_s > last.0 {
            return None;
        }
        // Right-most sample whose time is <= t.
        let idx = (0..self.profile.len() - 1)
            .rev()
            .find(|&i| self.profile[i].0 <= t_s)
            .unwrap_or(0);
        let (t0, s0) = self.profile[idx];
        let (t1, s1) = self.profile[idx + 1];
        if t1 == t0 {
            return Some(s0);
        }
        let frac = (t_s - t0) as f64 / (t1 - t0) as f64;
        Some(s0 + (s1 - s0) * frac)
    }
}

/// One observation from a rider's trace.
#[derive(Debug, Clone, Copy)]
pub struct ObsFix {
    /// Seconds after midnight of the trace's reference day.
    pub t_s: i64,
    /// Distance along the line in metres (from [`LineProject::project`]).
    pub s_m: f64,
    /// Reported horizontal accuracy in metres.
    pub accuracy_m: f64,
}

/// A candidate trip with its fitted delay and score.
#[derive(Debug, Clone)]
pub struct ScoredTrip {
    pub trip_id: String,
    /// Fitted delay in seconds (positive = late).
    pub delta_s: f64,
    /// Weighted mean squared residual, m^2.
    pub residual_m2: f64,
    /// Posterior probability among the scored candidates.
    pub posterior: f64,
}

/// What the matcher decided for one rider trace.
#[derive(Debug, Clone)]
pub enum RailDecision {
    /// Enough evidence: one trip is clearly the best explanation.
    Identified {
        trip_id: String,
        delay_s: f64,
        confidence: f64,
    },
    /// Ambiguous: the top trips are near-tied (plan §7.2 known ambiguity).
    Ambiguous { candidates: Vec<ScoredTrip> },
    /// Not enough evidence (too few fixes, or nothing fits).
    Insufficient { reason: String },
}

/// Rail matcher over one line's shape and its scheduled trips.
pub struct RailMatcher<'a> {
    /// The line shape. Kept so the matcher owns its geometry and callers can
    /// ask it for `length_m` / projections without a second argument.
    #[allow(dead_code)]
    shape: &'a LineProject,
    /// Candidate window half-width around "now", seconds (plan: ±45 min).
    window_s: i64,
    /// Plausible fitted-delay range, seconds (plan: -2 min .. +25 min).
    min_delta_s: i64,
    max_delta_s: i64,
    /// Acceptance: top posterior and margin over runner-up (plan §7.2 step 3).
    accept_posterior: f64,
    accept_margin: f64,
    /// Delay prior scale in seconds; asymmetric Laplace (late more likely).
    prior_scale_s: f64,
    /// A fit whose weighted mean-squared residual exceeds this many m^2 is not
    /// a plausible explanation at all, so the trace is rejected rather than
    /// reported as an uncertain match (plan §7.1 "accept on-rail" test).
    max_residual_m2: f64,
}

impl<'a> RailMatcher<'a> {
    pub fn new(shape: &'a LineProject) -> Self {
        Self {
            shape,
            window_s: 45 * 60,
            min_delta_s: -2 * 60,
            max_delta_s: 25 * 60,
            accept_posterior: 0.8,
            accept_margin: 2.0,
            prior_scale_s: 180.0,
            // ~50 m RMS: generous for GPS, far below a wrong-line fit.
            max_residual_m2: 2500.0,
        }
    }

    /// Override the candidate time window (seconds).
    pub fn with_window_s(mut self, window_s: i64) -> Self {
        self.window_s = window_s;
        self
    }

    /// Fit delay for one trip by minimising the weighted squared residual over
    /// `delta`: `argmin_delta sum_i w_i (s_obs(t_i) - s_sched(t_i - delta))^2`.
    ///
    /// A late train (delta > 0) means the rider sees at time `t` the position
    /// the schedule had at `t - delta`, hence the sign. Grid search at 1 s over
    /// the plausible range, then a parabolic refinement.
    fn fit_delay(&self, obs: &[ObsFix], trip: &TripTiming) -> Option<(f64, f64)> {
        if trip.profile.len() < 2 {
            return None;
        }
        // Plausible-delay search range. A rider's trace is only explicable by a
        // trip whose scheduled span covers it within this window: search
        // [obs_start - trip_end, obs_end - trip_start] so we never pick the
        // extreme of a wide grid just because the trace ends at a boundary.
        let obs_start = obs[0].t_s;
        let obs_end = obs[obs.len() - 1].t_s;
        let trip_start = trip.profile[0].0;
        let trip_end = trip.profile[trip.profile.len() - 1].0;
        let lo = (obs_start - trip_end).max(self.min_delta_s);
        let hi = (obs_end - trip_start).min(self.max_delta_s);
        if hi < lo {
            return None;
        }
        let mut best: Option<(f64, f64)> = None;
        let mut d = lo;
        while d <= hi {
            if let Some(r) = self.weighted_residual(obs, trip, d as f64) {
                if best.is_none_or(|(_, br)| r < br) {
                    best = Some((d as f64, r));
                }
            }
            d += 1;
        }
        let (mut delta, _) = best?;
        // Parabolic refinement around the grid minimum.
        let rm = self.weighted_residual(obs, trip, delta - 1.0);
        let r0 = self.weighted_residual(obs, trip, delta);
        let rp = self.weighted_residual(obs, trip, delta + 1.0);
        if let (Some(rm), Some(r0), Some(rp)) = (rm, r0, rp) {
            let denom = rm - 2.0 * r0 + rp;
            if denom.abs() > 1e-9 {
                delta += (rm - rp) / (2.0 * denom);
            }
        }
        let residual = self.weighted_residual(obs, trip, delta)?;
        Some((delta, residual))
    }

    fn weighted_residual(&self, obs: &[ObsFix], trip: &TripTiming, delta_s: f64) -> Option<f64> {
        let mut sum_w = 0.0;
        let mut sum_wr = 0.0;
        // `delta` is the fitted delay. A train that is *late* by delta seconds
        // reaches at time t the position the schedule reached at t - delta, so
        // we compare the observation against `s_sched(t - delta)`.
        let shift = delta_s.round() as i64;
        for f in obs {
            // A fix outside the trip's scheduled span contributes nothing to
            // this candidate rather than invalidating the whole fit.
            let Some(sched) = trip.s_at(f.t_s - shift) else {
                continue;
            };
            // Variance grows with reported accuracy; never divide by zero.
            let var = (f.accuracy_m * 2.0).max(1.0).powi(2);
            let w = 1.0 / var;
            let err = f.s_m - sched;
            sum_w += w;
            sum_wr += w * err * err;
        }
        if sum_w <= 0.0 {
            return None;
        }
        Some(sum_wr / sum_w)
    }

    /// Asymmetric delay prior: log-density. Late is more likely than early
    /// (plan §7.2 step 2). Roughly Laplace with a heavier tail in the late
    /// direction.
    fn log_prior(&self, delta_s: f64) -> f64 {
        let b = self.prior_scale_s;
        if delta_s >= 0.0 {
            -(delta_s / b)
        } else {
            -(delta_s.abs() / (b * 1.5))
        }
    }

    /// Score every trip in `trips` against the observations and decide.
    ///
    /// `now_s` (seconds after midnight) is used to prefer trips whose scheduled
    /// span actually covers the observation window, so a trace at 06:12 is not
    /// explained by a trip that finished at 05:30.
    pub fn decide(&self, obs: &[ObsFix], now_s: i64, trips: &[TripTiming]) -> RailDecision {
        if obs.len() < 3 {
            return RailDecision::Insufficient {
                reason: format!("{} fixes is too few", obs.len()),
            };
        }
        let mut scored: Vec<ScoredTrip> = Vec::new();
        for trip in trips {
            // Window gate: the trip must overlap the observed span within the
            // plausible delay range. This is what makes the search cheap and
            // stops far-away trips from scoring well by accident.
            let trip_start = trip.profile.first().map(|p| p.0).unwrap_or(trip.start_s);
            let trip_end = trip.profile.last().map(|p| p.0).unwrap_or(trip.start_s);
            let obs_start = obs[0].t_s;
            let obs_end = obs[obs.len() - 1].t_s;
            if obs_end + self.min_delta_s > trip_end + self.window_s
                || obs_start + self.max_delta_s < trip_start - self.window_s
            {
                continue;
            }
            let Some((delta, residual)) = self.fit_delay(obs, trip) else {
                continue;
            };
            // A poor fit is not a candidate at all: reject it outright so a
            // walker or an off-line trace cannot be reported as a weak match.
            if residual > self.max_residual_m2 {
                continue;
            }
            // Log-likelihood proxy: residual is m^2; normalise by a nominal
            // GPS sigma (30 m) and scale by the number of fixes.
            let log_like = -residual / (2.0 * 30.0 * 30.0) * obs.len() as f64;
            let log_post = log_like + self.log_prior(delta);
            scored.push(ScoredTrip {
                trip_id: trip.trip_id.clone(),
                delta_s: delta,
                residual_m2: residual,
                posterior: log_post,
            });
        }
        let _ = now_s;
        if scored.is_empty() {
            return RailDecision::Insufficient {
                reason: "no scheduled trip fits the observed track".into(),
            };
        }
        // Softmax over the log-posteriors to get normalised probabilities.
        let max = scored
            .iter()
            .map(|s| s.posterior)
            .fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = scored.iter().map(|s| (s.posterior - max).exp()).collect();
        let sum: f64 = exps.iter().sum();
        for (s, e) in scored.iter_mut().zip(exps) {
            s.posterior = e / sum;
        }
        scored.sort_by(|a, b| {
            b.posterior
                .partial_cmp(&a.posterior)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let top = &scored[0];
        let runner_up = scored.get(1).map(|s| s.posterior).unwrap_or(0.0);
        if top.posterior >= self.accept_posterior && top.posterior >= self.accept_margin * runner_up
        {
            RailDecision::Identified {
                trip_id: top.trip_id.clone(),
                delay_s: top.delta_s,
                confidence: top.posterior,
            }
        } else {
            RailDecision::Ambiguous {
                candidates: scored.into_iter().take(2).collect(),
            }
        }
    }
}
