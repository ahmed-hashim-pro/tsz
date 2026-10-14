//! Bit-level writer and reader, most significant bit first.

use crate::Error;

/// Where encoders put their bits. Every encoder is generic over this, so the
/// same code that writes a block can also just measure it: `Auto` counts each
/// candidate with a [`BitCounter`] and writes only the smallest.
pub trait Sink {
    /// Writes the low `bits` bits of `value` (0 <= bits <= 64).
    fn write(&mut self, value: u64, bits: u32);
    /// Bits written so far.
    fn len(&self) -> u64;
    /// True once a counter has passed its budget; encoders check it once per
    /// value and stop early. Always false for a writer.
    #[inline]
    fn full(&self) -> bool {
        false
    }
    #[inline]
    fn bit(&mut self, b: bool) {
        self.write(b as u64, 1);
    }
}

/// A sink that only counts, and gives up once it reaches `limit`.
pub struct BitCounter {
    n: u64,
    limit: u64,
}

impl BitCounter {
    pub fn with_limit(limit: u64) -> Self {
        Self { n: 0, limit }
    }
}

impl Sink for BitCounter {
    #[inline]
    fn write(&mut self, _value: u64, bits: u32) {
        self.n += bits as u64;
    }
    fn len(&self) -> u64 {
        self.n
    }
    #[inline]
    fn full(&self) -> bool {
        self.n >= self.limit
    }
}

/// Appends bits to a byte buffer, most significant bit first.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    n: u32, // bits pending in the low end of `acc`, always < 64 between calls
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn flush_word(&mut self) {
        self.buf.extend_from_slice(&self.acc.to_be_bytes());
        self.acc = 0;
        self.n = 0;
    }

    /// The bytes, with the last one zero-padded on the right.
    pub fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let word = (self.acc << (64 - self.n)).to_be_bytes();
            let bytes = self.n.div_ceil(8) as usize;
            self.buf.extend_from_slice(&word[..bytes]);
        }
        self.buf
    }
}

impl Sink for BitWriter {
    #[inline]
    fn write(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 64);
        if bits == 0 {
            return;
        }
        let value = if bits == 64 {
            value
        } else {
            value & ((1u64 << bits) - 1)
        };
        let free = 64 - self.n;
        if bits < free {
            self.acc = (self.acc << bits) | value;
            self.n += bits;
        } else if bits == free {
            self.acc = if bits == 64 {
                value
            } else {
                (self.acc << bits) | value
            };
            self.flush_word();
        } else {
            let rest = bits - free;
            self.acc = (self.acc << free) | (value >> rest);
            self.flush_word();
            self.acc = value & ((1u64 << rest) - 1);
            self.n = rest;
        }
    }

    fn len(&self) -> u64 {
        self.buf.len() as u64 * 8 + self.n as u64
    }
}

/// Reads bits written by [`BitWriter`]. Every read is bounds-checked: running
/// off the end is [`Error::Truncated`], never a panic.
pub struct BitReader<'a> {
    data: &'a [u8],
    next: usize, // next byte to load into `buf`
    buf: u64,    // unread bits, left-aligned; `avail` of them are counted
    avail: u32,
    used: u64, // bits consumed
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            next: 0,
            buf: 0,
            avail: 0,
            used: 0,
        }
    }

    // Tops `buf` up to at least 57 bits, or to the end of the data.
    #[inline]
    fn refill(&mut self) {
        if self.next + 8 <= self.data.len() {
            // Load 8 bytes and keep the whole ones that fit. The bits of a
            // partly fitting byte also land in `buf`, uncounted; they are the
            // stream's own bits, so the next load ORs the same values over them.
            let w = u64::from_be_bytes(self.data[self.next..self.next + 8].try_into().unwrap());
            self.buf |= w >> self.avail;
            let bytes = (63 - self.avail) / 8;
            self.next += bytes as usize;
            self.avail += bytes * 8;
        } else {
            while self.avail <= 56 && self.next < self.data.len() {
                self.buf |= (self.data[self.next] as u64) << (56 - self.avail);
                self.next += 1;
                self.avail += 8;
            }
        }
    }

    /// Reads `bits` bits (0 <= bits <= 64).
    #[inline]
    pub fn read(&mut self, bits: u32) -> Result<u64, Error> {
        debug_assert!(bits <= 64);
        if bits == 0 {
            return Ok(0);
        }
        if bits > 56 {
            let hi = self.read(bits - 32)?;
            return Ok(hi << 32 | self.read(32)?);
        }
        if self.avail < bits {
            self.refill();
            if self.avail < bits {
                return Err(Error::Truncated);
            }
        }
        let v = self.buf >> (64 - bits);
        self.buf <<= bits;
        self.avail -= bits;
        self.used += bits as u64;
        Ok(v)
    }

    #[inline]
    pub fn bit(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? == 1)
    }

    /// Counts leading one bits, up to `max` (at most 8), consuming them and
    /// the zero that ends the run (if the run stopped before `max`). Prefix
    /// codes in this crate are all of this unary shape.
    #[inline]
    pub fn unary(&mut self, max: u32) -> Result<u32, Error> {
        debug_assert!((1..=8).contains(&max));
        if self.avail < max {
            self.refill();
        }
        let k = (!self.buf).leading_zeros().min(max);
        let take = if k < max { k + 1 } else { max };
        if self.avail < take {
            return Err(Error::Truncated);
        }
        self.buf <<= take;
        self.avail -= take;
        self.used += take as u64;
        Ok(k)
    }

    /// Bytes touched so far, counting a partly read byte as whole.
    pub fn bytes_used(&self) -> usize {
        self.used.div_ceil(8) as usize
    }
}

#[inline]
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
pub fn unzigzag(z: u64) -> i64 {
    ((z >> 1) as i64) ^ -((z & 1) as i64)
}

/// Bits needed to hold `z` (0 for 0).
#[inline]
pub fn bit_len(z: u64) -> u32 {
    64 - z.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_round_trip() {
        let mut w = BitWriter::new();
        let mut expect = Vec::new();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for i in 0..2000u32 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let bits = i % 65;
            let v = if bits == 64 {
                x
            } else {
                x & ((1u64 << bits) - 1)
            };
            w.write(v, bits);
            expect.push((v, bits));
        }
        let total = w.len();
        let bytes = w.finish();
        assert_eq!(bytes.len() as u64, total.div_ceil(8));
        let mut r = BitReader::new(&bytes);
        for (v, bits) in expect {
            assert_eq!(r.read(bits).unwrap(), v, "width {bits}");
        }
        assert_eq!(r.bytes_used(), bytes.len());
    }

    #[test]
    fn reading_past_the_end_is_an_error() {
        let mut r = BitReader::new(&[0xff]);
        assert_eq!(r.read(8).unwrap(), 0xff);
        assert_eq!(r.read(1), Err(Error::Truncated));
        let mut r = BitReader::new(&[]);
        assert_eq!(r.read(64), Err(Error::Truncated));
        assert_eq!(r.unary(3), Err(Error::Truncated));
    }

    #[test]
    fn zigzag_is_a_bijection_at_the_edges() {
        for v in [0, 1, -1, 2, -2, i64::MAX, i64::MIN, i64::MIN + 1] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
        assert_eq!(zigzag(i64::MIN), u64::MAX);
    }
}
