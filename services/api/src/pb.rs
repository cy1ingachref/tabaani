//! Hand-written Rust mirror of `proto/tabaani/v1/transit.proto`.
//!
//! We keep the wire contract in protobuf (for the Flutter/Dart client and for
//! GTFS-RT interop) but decode it ourselves here. The structs below are the
//! exact field set of the `.proto`, with matching field numbers, so a
//! `prost`-generated client and this decoder agree on the wire format.
//!
//! Decoding is a small hand-rolled varint reader rather than a dependency: the
//! messages are tiny and the server must never fail on a malformed payload.

use serde::{Deserialize, Serialize};

/// Protobuf wire types we understand.
mod wire {
    pub const VARINT: u8 = 0;
    pub const FIXED64: u8 = 1;
    pub const LEN: u8 = 2;
    pub const FIXED32: u8 = 5;
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("truncated message")]
    Truncated,
    #[error("varint too long")]
    VarintTooLong,
    #[error("length delimiter overflows buffer")]
    BadLength,
    #[error("unexpected wire type {0}")]
    WireType(u8),
    #[error("invalid utf-8")]
    Utf8(#[from] std::str::Utf8Error),
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let b = *self.buf.get(self.pos).ok_or(DecodeError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    pub fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut out: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift >= 64 {
                return Err(DecodeError::VarintTooLong);
            }
            out |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
            if shift >= 70 {
                return Err(DecodeError::VarintTooLong);
            }
        }
    }

    pub fn tag(&mut self) -> Result<(u32, u8), DecodeError> {
        let key = self.varint()?;
        let field = (key >> 3) as u32;
        let wt = (key & 0x7) as u8;
        if field == 0 {
            return Err(DecodeError::WireType(wt));
        }
        Ok((field, wt))
    }

    pub fn len_delimited(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.varint()? as usize;
        let end = self.pos.checked_add(n).ok_or(DecodeError::BadLength)?;
        if end > self.buf.len() {
            return Err(DecodeError::BadLength);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// zigzag-decoded sint32.
    pub fn sint32(&mut self) -> Result<i32, DecodeError> {
        let n = self.varint()?;
        Ok(zigzag_decode32(n))
    }

    pub fn uint32(&mut self) -> Result<u32, DecodeError> {
        Ok(self.varint()? as u32)
    }

    pub fn float(&mut self) -> Result<f32, DecodeError> {
        // Fixed32: read the four little-endian bytes directly.
        let mut bytes = [0u8; 4];
        for b in bytes.iter_mut() {
            *b = self.byte()?;
        }
        Ok(f32::from_le_bytes(bytes))
    }

    pub fn skip(&mut self, wt: u8) -> Result<(), DecodeError> {
        match wt {
            wire::VARINT => {
                let _ = self.varint()?;
            }
            wire::FIXED64 => {
                for _ in 0..8 {
                    let _ = self.byte()?;
                }
            }
            wire::LEN => {
                let _ = self.len_delimited()?;
            }
            wire::FIXED32 => {
                for _ in 0..4 {
                    let _ = self.byte()?;
                }
            }
            other => return Err(DecodeError::WireType(other)),
        }
        Ok(())
    }

    pub fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }
}

fn zigzag_decode32(n: u64) -> i32 {
    let v = n as u32;
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

fn zigzag_encode32(v: i32) -> u64 {
    (((v << 1) ^ (v >> 31)) as u32) as u64
}

/// Write a varint into `out`.
fn put_varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

fn put_tag(out: &mut Vec<u8>, field: u32, wt: u8) {
    put_varint(out, ((field as u64) << 3) | wt as u64);
}

fn put_uint32(out: &mut Vec<u8>, field: u32, v: u32) {
    if v == 0 {
        return;
    }
    put_tag(out, field, wire::VARINT);
    put_varint(out, v as u64);
}

fn put_sint32(out: &mut Vec<u8>, field: u32, v: i32) {
    put_tag(out, field, wire::VARINT);
    put_varint(out, zigzag_encode32(v));
}

fn put_float(out: &mut Vec<u8>, field: u32, v: f32) {
    if v == 0.0 {
        return;
    }
    put_tag(out, field, wire::FIXED32);
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, field: u32, s: &str) {
    if s.is_empty() {
        return;
    }
    put_tag(out, field, wire::LEN);
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

// --- messages ---------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fix {
    pub d_lat_e7: i32,
    pub d_lon_e7: i32,
    pub d_ts_ms: u32,
    pub accuracy_dm: u32,
    pub speed_cms: u32,
    pub bearing_deg: u32,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Hint {
    pub route_id: String,
    pub direction: u32,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct PingBatch {
    pub session_token: String,
    pub seq_start: u32,
    pub dataset_version: String,
    pub fixes: Vec<Fix>,
    pub hint: Option<Hint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    #[default]
    Unknown = 0,
    Scheduled = 1,
    Estimated = 2,
    Live = 3,
}

impl Confidence {
    pub fn from_wire(v: u32) -> Self {
        match v {
            1 => Self::Scheduled,
            2 => Self::Estimated,
            3 => Self::Live,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct VehicleState {
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

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LiveFrame {
    pub vehicles: Vec<VehicleState>,
}

impl PingBatch {
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let mut m = PingBatch::default();
        while !r.done() {
            let (field, wt) = r.tag()?;
            match (field, wt) {
                (1, wire::LEN) => m.session_token = std::str::from_utf8(r.len_delimited()?)?.into(),
                (2, wire::VARINT) => m.seq_start = r.uint32()?,
                (3, wire::LEN) => {
                    m.dataset_version = std::str::from_utf8(r.len_delimited()?)?.into()
                }
                (4, wire::LEN) => m.fixes.push(decode_fix(r.len_delimited()?)?),
                (5, wire::LEN) => m.hint = Some(decode_hint(r.len_delimited()?)?),
                _ => r.skip(wt)?,
            }
        }
        Ok(m)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.fixes.len() * 12);
        put_string(&mut out, 1, &self.session_token);
        put_uint32(&mut out, 2, self.seq_start);
        put_string(&mut out, 3, &self.dataset_version);
        for f in &self.fixes {
            let mut inner = Vec::new();
            put_sint32(&mut inner, 1, f.d_lat_e7);
            put_sint32(&mut inner, 2, f.d_lon_e7);
            put_uint32(&mut inner, 3, f.d_ts_ms);
            put_uint32(&mut inner, 4, f.accuracy_dm);
            put_uint32(&mut inner, 5, f.speed_cms);
            put_uint32(&mut inner, 6, f.bearing_deg);
            put_tag(&mut out, 4, wire::LEN);
            put_varint(&mut out, inner.len() as u64);
            out.extend_from_slice(&inner);
        }
        out
    }
}

fn decode_fix(buf: &[u8]) -> Result<Fix, DecodeError> {
    let mut r = Reader::new(buf);
    let mut f = Fix::default();
    while !r.done() {
        let (field, wt) = r.tag()?;
        match (field, wt) {
            (1, wire::VARINT) => f.d_lat_e7 = r.sint32()?,
            (2, wire::VARINT) => f.d_lon_e7 = r.sint32()?,
            (3, wire::VARINT) => f.d_ts_ms = r.uint32()?,
            (4, wire::VARINT) => f.accuracy_dm = r.uint32()?,
            (5, wire::VARINT) => f.speed_cms = r.uint32()?,
            (6, wire::VARINT) => f.bearing_deg = r.uint32()?,
            _ => r.skip(wt)?,
        }
    }
    Ok(f)
}

fn decode_hint(buf: &[u8]) -> Result<Hint, DecodeError> {
    let mut r = Reader::new(buf);
    let mut h = Hint::default();
    while !r.done() {
        let (field, wt) = r.tag()?;
        match (field, wt) {
            (1, wire::LEN) => h.route_id = std::str::from_utf8(r.len_delimited()?)?.into(),
            (2, wire::VARINT) => h.direction = r.uint32()?,
            _ => r.skip(wt)?,
        }
    }
    Ok(h)
}

impl LiveFrame {
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let mut frame = LiveFrame::default();
        while !r.done() {
            let (field, wt) = r.tag()?;
            match (field, wt) {
                (1, wire::LEN) => frame.vehicles.push(decode_vehicle(r.len_delimited()?)?),
                _ => r.skip(wt)?,
            }
        }
        Ok(frame)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for v in &self.vehicles {
            let mut inner = Vec::new();
            put_string(&mut inner, 1, &v.vehicle_id);
            put_string(&mut inner, 2, &v.route_id);
            put_uint32(&mut inner, 3, v.direction);
            put_float(&mut inner, 4, v.s_m);
            put_float(&mut inner, 5, v.v_mps);
            if v.ts_ms != 0 {
                put_tag(&mut inner, 6, wire::VARINT);
                put_varint(&mut inner, v.ts_ms);
            }
            put_uint32(&mut inner, 7, v.confidence as u32);
            put_uint32(&mut inner, 8, v.n_riders);
            put_string(&mut inner, 9, &v.trip_id);
            put_float(&mut inner, 10, v.delay_s);
            put_string(&mut inner, 11, &v.vehicle_type);
            put_uint32(&mut inner, 12, v.n_cars);
            put_tag(&mut out, 1, wire::LEN);
            put_varint(&mut out, inner.len() as u64);
            out.extend_from_slice(&inner);
        }
        out
    }
}

fn decode_vehicle(buf: &[u8]) -> Result<VehicleState, DecodeError> {
    let mut r = Reader::new(buf);
    let mut v = VehicleState::default();
    while !r.done() {
        let (field, wt) = r.tag()?;
        match (field, wt) {
            (1, wire::LEN) => v.vehicle_id = std::str::from_utf8(r.len_delimited()?)?.into(),
            (2, wire::LEN) => v.route_id = std::str::from_utf8(r.len_delimited()?)?.into(),
            (3, wire::VARINT) => v.direction = r.uint32()?,
            (4, wire::FIXED32) => v.s_m = read_fixed32(&mut r)?,
            (5, wire::FIXED32) => v.v_mps = read_fixed32(&mut r)?,
            (6, wire::VARINT) => v.ts_ms = r.varint()?,
            (7, wire::VARINT) => v.confidence = Confidence::from_wire(r.uint32()?),
            (8, wire::VARINT) => v.n_riders = r.uint32()?,
            (9, wire::LEN) => v.trip_id = std::str::from_utf8(r.len_delimited()?)?.into(),
            (10, wire::FIXED32) => v.delay_s = read_fixed32(&mut r)?,
            (11, wire::LEN) => v.vehicle_type = std::str::from_utf8(r.len_delimited()?)?.into(),
            (12, wire::VARINT) => v.n_cars = r.uint32()?,
            _ => r.skip(wt)?,
        }
    }
    Ok(v)
}

fn read_fixed32(r: &mut Reader<'_>) -> Result<f32, DecodeError> {
    let mut b = [0u8; 4];
    for x in b.iter_mut() {
        *x = r.byte()?;
    }
    Ok(f32::from_le_bytes(b))
}
