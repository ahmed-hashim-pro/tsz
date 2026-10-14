//! Gorilla (Pelkonen et al., VLDB 2015): delta-of-delta timestamps and XOR
//! floats.
//!
//! Two departures from the paper, both so that every i64 timestamp and every
//! f64 bit pattern round-trips:
//!
//! - The paper's widest timestamp bucket is 32 bits. Here a fifth bucket
//!   carries a full 64-bit delta-of-delta, so jumps of any size, timestamps
//!   that go backwards, and duplicates all encode. Arithmetic wraps, so
//!   i64::MIN next to i64::MAX is fine too.
//! - The paper's first delta is 14 bits. Here it goes through the same
//!   buckets, with the delta before it taken as 0.

use crate::bits::{bit_len, unzigzag, zigzag, BitReader, Sink};
use crate::Error;

// (prefix bits, prefix value, payload bits) for the non-zero buckets.
// A zero delta-of-delta is the single bit '0'.
const TS_BUCKETS: [(u32, u64, u32); 5] = [
    (2, 0b10, 7),
    (3, 0b110, 9),
    (4, 0b1110, 12),
    (5, 0b11110, 32),
    (5, 0b11111, 64),
];

pub fn encode_ts(ts: &[i64], w: &mut impl Sink) {
    let Some(&first) = ts.first() else { return };
    w.write(first as u64, 64);
    let (mut prev, mut prev_delta) = (first, 0i64);
    for &t in &ts[1..] {
        if w.full() {
            return;
        }
        let delta = t.wrapping_sub(prev);
        let z = zigzag(delta.wrapping_sub(prev_delta));
        if z == 0 {
            w.bit(false);
        } else {
            let len = bit_len(z);
            let &(pb, pv, payload) = TS_BUCKETS.iter().find(|b| len <= b.2).unwrap();
            w.write(pv, pb);
            w.write(z, payload);
        }
        prev = t;
        prev_delta = delta;
    }
}

pub fn decode_ts(r: &mut BitReader, n: usize, out: &mut Vec<i64>) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let first = r.read(64)? as i64;
    out.push(first);
    let (mut prev, mut prev_delta) = (first, 0i64);
    for _ in 1..n {
        let k = r.unary(5)?;
        let dod = if k == 0 {
            0
        } else {
            let payload = TS_BUCKETS[k as usize - 1].2;
            unzigzag(r.read(payload)?)
        };
        let delta = prev_delta.wrapping_add(dod);
        let t = prev.wrapping_add(delta);
        out.push(t);
        prev = t;
        prev_delta = delta;
    }
    Ok(())
}

pub fn encode_values(vs: &[u64], w: &mut impl Sink) {
    let Some(&first) = vs.first() else { return };
    w.write(first, 64);
    let mut prev = first;
    // The current window: leading and trailing zeros of the last XOR whose
    // bits were written in full. None until the first one.
    let mut window: Option<(u32, u32)> = None;
    for &v in &vs[1..] {
        if w.full() {
            return;
        }
        let xor = v ^ prev;
        prev = v;
        if xor == 0 {
            w.bit(false);
            continue;
        }
        let lz = xor.leading_zeros().min(31);
        let tz = xor.trailing_zeros();
        match window {
            Some((wl, wt)) if lz >= wl && tz >= wt => {
                w.write(0b10, 2);
                w.write(xor >> wt, 64 - wl - wt);
            }
            _ => {
                let sig = 64 - lz - tz;
                w.write(0b11, 2);
                w.write(lz as u64, 5);
                w.write(sig as u64 & 63, 6); // 64 is stored as 0
                w.write(xor >> tz, sig);
                window = Some((lz, tz));
            }
        }
    }
}

pub fn decode_values(r: &mut BitReader, n: usize, out: &mut Vec<f64>) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let mut prev = r.read(64)?;
    out.push(f64::from_bits(prev));
    let mut window: Option<(u32, u32)> = None;
    for _ in 1..n {
        if r.bit()? {
            let xor = if r.bit()? {
                let lz = r.read(5)? as u32;
                let sig = match r.read(6)? as u32 {
                    0 => 64,
                    s => s,
                };
                if lz + sig > 64 {
                    return Err(Error::Corrupt("gorilla window wider than 64 bits"));
                }
                let tz = 64 - lz - sig;
                window = Some((lz, tz));
                r.read(sig)? << tz
            } else {
                let (wl, wt) = window.ok_or(Error::Corrupt("gorilla window reused before set"))?;
                r.read(64 - wl - wt)? << wt
            };
            prev ^= xor;
        }
        out.push(f64::from_bits(prev));
    }
    Ok(())
}
