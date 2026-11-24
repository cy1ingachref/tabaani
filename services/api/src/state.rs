//! Shared Redis live-state representation.
//!
//! The matcher writes one JSON blob per vehicle under `veh:{id}` and registers
//! the id in the `veh:ids` set. This module is the single place that knows the
//! shape, so the api and matcher cannot drift apart.

use serde::{Deserialize, Serialize};

use crate::pb::{Confidence, VehicleState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVehicle {
    pub vehicle_id: String,
    pub route_id: String,
    pub direction: u32,
    pub s_m: f32,
    pub v_mps: f32,
    pub ts_ms: u64,
    pub confidence: Confidence,
    pub n_riders: u32,
    pub trip_id: String,
    pub delay_s: f32,
    pub vehicle_type: String,
    pub n_cars: u32,
}

impl StoredVehicle {
    pub fn into_state(self) -> VehicleState {
        VehicleState {
            vehicle_id: self.vehicle_id,
            route_id: self.route_id,
            direction: self.direction,
            s_m: self.s_m,
            v_mps: self.v_mps,
            ts_ms: self.ts_ms,
            confidence: self.confidence,
            n_riders: self.n_riders,
            trip_id: self.trip_id,
            delay_s: self.delay_s,
            vehicle_type: self.vehicle_type,
            n_cars: self.n_cars,
        }
    }

    pub fn from_state(v: &VehicleState) -> Self {
        Self {
            vehicle_id: v.vehicle_id.clone(),
            route_id: v.route_id.clone(),
            direction: v.direction,
            s_m: v.s_m,
            v_mps: v.v_mps,
            ts_ms: v.ts_ms,
            confidence: v.confidence,
            n_riders: v.n_riders,
            trip_id: v.trip_id.clone(),
            delay_s: v.delay_s,
            vehicle_type: v.vehicle_type.clone(),
            n_cars: v.n_cars,
        }
    }
}
