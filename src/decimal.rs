//! The decimal codec: values that were printed with a few decimal places.
//!
//! Most metrics reach a database as text. A tide gauge reports `1.327`, a
//! power meter `2.580`, a counter `12472874`. Parsed to f64, `1.327` is the
//! double nearest 1327/1000, and its 52-bit mantissa is a repeating binary
//! fraction, so the XOR of two neighbouring readings is mostly noise. That is
//! where Gorilla and Chimp lose their bits.
//!
//! If every value in a block equals `n / 10^d` for an integer `n` and a small
//! `d`, checked bit for bit, the block can store the integers instead, through
//! the adaptive integer coder. Division is correctly rounded and so is
//! parsing, so `1327.0 / 1000.0` is exactly the double that `"1.327"` parses
//! to; the encoder still checks every value, and a block with one value that
//! doesn't fit (a NaN, a -0.0, a reading with an extra digit) falls back to a
//! float codec.

use crate::bits::{BitReader, Sink};
use crate::{intcode, Error};

/// Decimal places tried, 0 through MAX_DIGITS.
pub const MAX_DIGITS: u32 = 15;

const POW10: [f64; MAX_DIGITS as usize + 1] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15,
];

// Integers up to 2^53 convert to f64 exactly.
const EXACT: f64 = 9_007_199_254_740_992.0;

#[inline]
fn scaled(v: f64, d: u32) -> Option<i64> {
    let x = (v * POW10[d as usize]).round();
    if x.is_nan() || x.abs() > EXACT {
        return None; // also rejects infinities
    }
    let n = x as i64;
    ((n as f64 / POW10[d as usize]).to_bits() == v.to_bits()).then_some(n)
}

/// The fewest decimal places that represent every value exactly, if any,
/// with the integers for that many places left in `ints`.
pub fn digits(vs: &[f64], ints: &mut Vec<i64>) -> Option<u32> {
    'places: for d in 0..=MAX_DIGITS {
        ints.clear();
        for &v in vs {
            match scaled(v, d) {
                Some(n) => ints.push(n),
                None => continue 'places,
            }
        }
        return Some(d);
    }
    None
}

/// Writes integers from [`digits`] and their number of decimal places.
pub fn encode(d: u32, ints: &[i64], w: &mut impl Sink, scratch: &mut Vec<u64>) {
    w.write(d as u64, 4);
    intcode::encode(ints, w, scratch);
}

pub fn decode(
    r: &mut BitReader,
    n: usize,
    out: &mut Vec<f64>,
    ints: &mut Vec<i64>,
) -> Result<(), Error> {
    if n == 0 {
        return Ok(());
    }
    let d = r.read(4)? as usize;
    ints.clear();
    intcode::decode(r, n, ints)?;
    out.extend(ints.iter().map(|&i| i as f64 / POW10[d]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_printed_precision() {
        let parse = |s: &[&str]| s.iter().map(|x| x.parse().unwrap()).collect::<Vec<f64>>();
        let mut ints = Vec::new();
        let mut digits = |vs: &[f64]| digits(vs, &mut ints);
        assert_eq!(digits(&parse(&["1.327", "1.297", "1.2"])), Some(3));
        assert_eq!(digits(&parse(&["12472874", "0", "-3"])), Some(0));
        assert_eq!(digits(&parse(&["0.1", "0.2", "0.30000000000000004"])), None);
        assert_eq!(digits(&[f64::NAN]), None);
        assert_eq!(digits(&[-0.0]), None);
        assert_eq!(digits(&[f64::INFINITY]), None);
        assert_eq!(digits(&[1e300]), None);
        assert_eq!(digits(&parse(&["2.5", "-1"])), Some(1));
        assert_eq!(ints, [25, -10]);
    }
}
