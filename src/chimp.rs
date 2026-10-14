//! Chimp and Chimp128 (Liakos, Papakonstantinopoulou, Kotidis, VLDB 2022).
//!
//! Both XOR each value with an earlier one, like Gorilla, but spend the bits
//! differently. Gorilla's XORs of real data mostly have few trailing zeros,
//! so Chimp only pays for a trailing-zero count when there are more than six
//! of them, and rounds the leading-zero count to one of eight values so it
//! fits in three bits.
//!
//! Chimp128 also lets each value pick its reference from the previous 128,
//! choosing one that shares its low 14 bits when there is one. Real series
//! revisit the same values, and an exact repeat costs 9 bits.

use crate::bits::{BitReader, Sink};
use crate::Error;

const LEAD_DECODE: [u32; 8] = [0, 8, 12, 16, 18, 20, 22, 24];

// Leading zeros rounded down to the nearest representable count, as a 3-bit code.
#[inline]
fn lead_code(lz: u32) -> u32 {
    match lz {
        0..=7 => 0,
        8..=11 => 1,
        12..=15 => 2,
        16..=17 => 3,
        18..=19 => 4,
        20..=21 => 5,
        22..=23 => 6,
        _ => 7,
    }
}

/// How the bits of a non-zero XOR with few trailing zeros are written, shared
/// by both variants: '10' reuses the previous leading-zero count, '11' sets a
/// new one.
#[inline]
fn write_center(w: &mut impl Sink, xor: u64, stored: &mut u32) {
    let code = lead_code(xor.leading_zeros());
    let lead = LEAD_DECODE[code as usize];
    if lead == *stored {
        w.write(0b10, 2);
    } else {
        w.write(0b11 << 3 | code as u64, 5);
        *stored = lead;
    }
    w.write(xor, 64 - lead);
}

// A non-zero XOR with many trailing zeros: the 3-bit lead code, the 6-bit
// count of significant bits, then those bits.
#[inline]
fn write_trailing(w: &mut impl Sink, xor: u64) {
    let code = lead_code(xor.leading_zeros());
    let tz = xor.trailing_zeros();
    let sig = 64 - LEAD_DECODE[code as usize] - tz;
    w.write((code as u64) << 6 | sig as u64, 9);
    w.write(xor >> tz, sig);
}

#[inline]
fn read_trailing(r: &mut BitReader) -> Result<u64, Error> {
    let lead = LEAD_DECODE[r.read(3)? as usize];
    let sig = r.read(6)? as u32;
    if sig == 0 || lead + sig > 64 {
        return Err(Error::Corrupt("chimp significant-bit count out of range"));
    }
    Ok(r.read(sig)? << (64 - lead - sig))
}

#[inline]
fn read_center(r: &mut BitReader, flag: u64, stored: &mut u32) -> Result<u64, Error> {
    if flag == 0b11 {
        *stored = LEAD_DECODE[r.read(3)? as usize];
    } else if *stored > 64 {
        return Err(Error::Corrupt("chimp leading zeros reused before set"));
    }
    r.read(64 - *stored)
}

const NONE: u32 = 65; // "no stored leading-zero count"

pub fn encode(vs: &[u64], w: &mut impl Sink) {
    let Some(&first) = vs.first() else { return };
    w.write(first, 64);
    let mut prev = first;
    let mut stored = NONE;
    for &v in &vs[1..] {
        if w.full() {
            return;
        }
        let xor = v ^ prev;
        prev = v;
        if xor == 0 {
            w.write(0b00, 2);
            stored = NONE;
        } else if xor.trailing_zeros() > 6 {
            w.write(0b01, 2);
            write_trailing(w, xor);
            stored = NONE;
        } else {
            write_center(w, xor, &mut stored);
        }
    }
}

pub fn decode(r: &mut BitReader, n: usize, out: &mut Vec<f64>) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let mut prev = r.read(64)?;
    out.push(f64::from_bits(prev));
    let mut stored = NONE;
    for _ in 1..n {
        match r.read(2)? {
            0b00 => stored = NONE,
            0b01 => {
                prev ^= read_trailing(r)?;
                stored = NONE;
            }
            flag => prev ^= read_center(r, flag, &mut stored)?,
        }
        out.push(f64::from_bits(prev));
    }
    Ok(())
}

const WINDOW: usize = 128;
const WINDOW_BITS: u32 = 7;
const THRESHOLD: u32 = 6 + WINDOW_BITS;
const KEY_BITS: u32 = THRESHOLD + 1;

/// Chimp128's encoder keeps a table from the low 14 bits of a value to where
/// that value was last seen. It is reused across blocks, so it lives here.
pub struct Chimp128Table {
    last_seen: Vec<u32>, // index + 1 within the block, 0 for never
}

impl Default for Chimp128Table {
    fn default() -> Self {
        Self {
            last_seen: vec![0; 1 << KEY_BITS],
        }
    }
}

#[inline]
fn key(v: u64) -> usize {
    (v & ((1 << KEY_BITS) - 1)) as usize
}

pub fn encode128(vs: &[u64], w: &mut impl Sink, table: &mut Chimp128Table) {
    let Some(&first) = vs.first() else { return };
    w.write(first, 64);
    table.last_seen[key(first)] = 1;
    let mut stored = NONE;
    for i in 1..vs.len() {
        if w.full() {
            // Undo this block's table entries, as the normal exit does.
            for &v in &vs[..i] {
                table.last_seen[key(v)] = 0;
            }
            return;
        }
        let v = vs[i];
        let mut slot = (i - 1) % WINDOW;
        let mut xor = v ^ vs[i - 1];
        let seen = table.last_seen[key(v)] as usize;
        if seen != 0 && i - (seen - 1) <= WINDOW {
            let candidate = v ^ vs[seen - 1];
            if candidate.trailing_zeros() > THRESHOLD {
                slot = (seen - 1) % WINDOW;
                xor = candidate;
            }
        }
        if xor == 0 {
            w.write(slot as u64, 2 + WINDOW_BITS);
            stored = NONE;
        } else if xor.trailing_zeros() > THRESHOLD {
            w.write((0b01 << WINDOW_BITS) | slot as u64, 2 + WINDOW_BITS);
            write_trailing(w, xor);
            stored = NONE;
        } else {
            // A candidate is only taken when it gives more than THRESHOLD
            // trailing zeros, so here the reference is always the previous
            // value and needs no index.
            write_center(w, xor, &mut stored);
        }
        table.last_seen[key(v)] = i as u32 + 1;
    }
    for &v in vs {
        table.last_seen[key(v)] = 0;
    }
}

pub fn decode128(r: &mut BitReader, n: usize, out: &mut Vec<f64>) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let mut ring = [0u64; WINDOW];
    let first = r.read(64)?;
    ring[0] = first;
    out.push(f64::from_bits(first));
    let mut stored = NONE;
    for i in 1..n {
        let prev = ring[(i - 1) % WINDOW];
        let v = match r.read(2)? {
            flag @ (0b00 | 0b01) => {
                let slot = r.read(WINDOW_BITS)? as usize;
                // A slot not yet written in this block can only come from corrupt input.
                if slot >= i {
                    return Err(Error::Corrupt("chimp128 reference before the block start"));
                }
                stored = NONE;
                if flag == 0b00 {
                    ring[slot]
                } else {
                    ring[slot] ^ read_trailing(r)?
                }
            }
            flag => prev ^ read_center(r, flag, &mut stored)?,
        };
        ring[i % WINDOW] = v;
        out.push(f64::from_bits(v));
    }
    Ok(())
}
