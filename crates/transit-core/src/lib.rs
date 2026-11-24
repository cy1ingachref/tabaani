//! Shared transit geometry and matching maths.
//!
//! Deliberately dependency-free: the matcher hot path must not pull in a geo
//! stack. Everything here works on WGS84 lon/lat degrees converted to local
//! metres, which is accurate to well under a metre over line-length extents.

pub mod geometry;
pub mod rail;
#[cfg(test)]
mod tests;

pub use geometry::{LineProject, Projection};
pub use rail::{RailDecision, RailMatcher, ScoredTrip, TripTiming};
