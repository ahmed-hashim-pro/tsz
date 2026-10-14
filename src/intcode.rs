//! An integer coder whose bucket widths are fitted to each block.
//!
//! Gorilla's timestamp buckets are fixed at 7, 9, 12 and 32 bits, which suits
//! a scrape interval with a little jitter and nothing else. Here the residual
//! (first or second difference, zigzagged) gets a four-way prefix code:
//!
//! ```text
//! 0                     residual is 0
//! 10  + w1 bits         fits in w1 bits
//! 110 + w2 bits         fits in w2 bits
//! 111 + 6 + len bits    anything else: its length, then its bits
//! ```
//!
//! and the block header carries the order (1 or 2) and w1, w2. The encoder
//! tries both orders and every (w1, w2), which is cheap: the cost of a choice
//! only depends on the histogram of residual lengths.

use crate::bits::{bit_len, unzigzag, zigzag, BitReader, Sink};
use crate::Error;

struct Plan {
    order: u32,
    w1: u32,
    w2: u32,
    bits: u64,
}

fn residuals(xs: &[i64], order: u32, out: &mut Vec<u64>) {
    out.clear();
    let mut prev_delta = 0i64;
    for pair in xs.windows(2) {
        let delta = pair[1].wrapping_sub(pair[0]);
        out.push(match order {
            1 => zigzag(delta),
            _ => zigzag(delta.wrapping_sub(prev_delta)),
        });
        prev_delta = delta;
    }
}

fn best_widths(rs: &[u64]) -> (u32, u32, u64) {
    let mut hist = [0u64; 65];
    for &r in rs {
        hist[bit_len(r) as usize] += 1;
    }
    // count[l] = residuals of length 1..=l; tail[l] = escape cost of lengths > l.
    let mut count = [0u64; 65];
    let mut tail = [0u64; 66];
    for l in 1..=64 {
        count[l] = count[l - 1] + hist[l];
    }
    for l in (1..=64).rev() {
        tail[l - 1] = tail[l] + hist[l] * (3 + 6 + l as u64);
    }
    let mut best = (1, 64, u64::MAX);
    for w1 in 1..64u32 {
        let c1 = count[w1 as usize] * (2 + w1 as u64);
        for w2 in w1 + 1..=64u32 {
            let c2 = (count[w2 as usize] - count[w1 as usize]) * (3 + w2 as u64);
            let cost = c1 + c2 + tail[w2 as usize];
            if cost < best.2 {
                best = (w1, w2, cost);
            }
        }
    }
    (best.0, best.1, best.2 + hist[0])
}

fn plan(xs: &[i64], scratch: &mut Vec<u64>) -> Plan {
    let mut best: Option<Plan> = None;
    for order in [1, 2] {
        residuals(xs, order, scratch);
        let (w1, w2, bits) = best_widths(scratch);
        let better = match &best {
            Some(b) => bits < b.bits,
            None => true,
        };
        if better {
            best = Some(Plan {
                order,
                w1,
                w2,
                bits,
            });
        }
    }
    best.unwrap()
}

pub fn encode(xs: &[i64], w: &mut impl Sink, scratch: &mut Vec<u64>) {
    let Some(&first) = xs.first() else { return };
    w.write(first as u64, 64);
    if xs.len() == 1 {
        return;
    }
    let p = plan(xs, scratch);
    w.write(p.order as u64 - 1, 1);
    w.write(p.w1 as u64, 6);
    w.write(p.w2 as u64, 7);
    residuals(xs, p.order, scratch);
    for &r in scratch.iter() {
        if w.full() {
            return;
        }
        let len = bit_len(r);
        if len == 0 {
            w.bit(false);
        } else if len <= p.w1 {
            w.write(0b10, 2);
            w.write(r, p.w1);
        } else if len <= p.w2 {
            w.write(0b110, 3);
            w.write(r, p.w2);
        } else {
            w.write(0b111, 3);
            w.write(len as u64 - 1, 6);
            w.write(r, len);
        }
    }
}

pub fn decode(r: &mut BitReader, n: usize, out: &mut Vec<i64>) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let first = r.read(64)? as i64;
    out.push(first);
    if n == 1 {
        return Ok(());
    }
    let order = r.read(1)? + 1;
    let w1 = r.read(6)? as u32;
    let w2 = r.read(7)? as u32;
    if w1 == 0 || w2 <= w1 || w2 > 64 {
        return Err(Error::Corrupt("integer code widths out of range"));
    }
    let (mut prev, mut prev_delta) = (first, 0i64);
    for _ in 1..n {
        let z = match r.unary(3)? {
            0 => 0,
            1 => r.read(w1)?,
            2 => r.read(w2)?,
            _ => {
                let len = r.read(6)? as u32 + 1;
                r.read(len)?
            }
        };
        let delta = if order == 1 {
            unzigzag(z)
        } else {
            prev_delta.wrapping_add(unzigzag(z))
        };
        let x = prev.wrapping_add(delta);
        out.push(x);
        prev = x;
        prev_delta = delta;
    }
    Ok(())
}
