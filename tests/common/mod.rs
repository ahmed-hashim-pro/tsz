//! Seeded generators for adversarial series. Every case is reproducible from
//! its seed, which failing assertions print.
#![allow(dead_code)]

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    // SplitMix64
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo) as u64 + 1) as i64
    }
    pub fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Values that break naive float handling: every NaN class with payloads,
/// both zeros, both infinities, subnormals, and the extremes.
pub fn special(r: &mut Rng) -> f64 {
    let payload = r.next() & ((1 << 52) - 1);
    let sign = r.next() << 63;
    f64::from_bits(match r.below(10) {
        0 => 0x7ff8_0000_0000_0000 | payload, // quiet NaN, any payload
        1 => 0x7ff0_0000_0000_0000 | payload.max(1), // signalling NaN
        2 => sign | 0x7ff0_0000_0000_0000 | payload.max(1),
        3 => 0,
        4 => 1 << 63,                      // -0.0
        5 => sign | 0x7ff0_0000_0000_0000, // ±inf
        6 => sign | payload,               // subnormal (or zero)
        7 => sign | f64::MAX.to_bits(),
        8 => sign | f64::MIN_POSITIVE.to_bits(),
        _ => sign | f64::EPSILON.to_bits(),
    })
}

fn segment(r: &mut Rng, n: usize, out: &mut Vec<f64>) {
    match r.below(8) {
        0 => out.extend((0..n).map(|_| f64::from_bits(r.next()))),
        1 => out.extend((0..n).map(|_| {
            if r.chance(2) {
                special(r)
            } else {
                f64::from_bits(r.next())
            }
        })),
        2 => {
            // Printed decimals, as a sensor reports them, with the odd outlier.
            let p = 10f64.powi(r.range(0, 6) as i32);
            let step = r.range(1, 1000);
            let mut k = r.range(-1_000_000, 1_000_000);
            for _ in 0..n {
                k += r.range(-step, step);
                out.push(if r.chance(60) {
                    special(r)
                } else {
                    k as f64 / p
                });
            }
        }
        3 => {
            let mut v = f64::from_bits(r.next());
            for _ in 0..n {
                if r.chance(20) {
                    v = if r.chance(3) {
                        special(r)
                    } else {
                        r.unit() * 100.0
                    };
                }
                out.push(v);
            }
        }
        4 => {
            let (a, w, b) = (r.unit() * 1e3, r.unit(), r.unit() * 1e6 - 5e5);
            out.extend((0..n).map(|i| a * (i as f64 * w).sin() + b));
        }
        5 => {
            // A small palette, revisited: Chimp128's case.
            let k = r.range(1, 200) as usize;
            let palette: Vec<f64> = (0..k).map(|_| f64::from_bits(r.next())).collect();
            out.extend((0..n).map(|_| palette[r.below(k as u64) as usize]));
        }
        6 => {
            let mut c = r.range(-1 << 40, 1 << 40);
            for _ in 0..n {
                let hi = 1 << r.below(50);
                c += r.range(-5, hi);
                out.push(c as f64);
            }
        }
        _ => out.extend((0..n).map(|_| special(r))),
    }
}

pub fn values(r: &mut Rng, n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let len = if r.chance(3) {
            n - out.len()
        } else {
            r.range(1, (n - out.len()) as i64) as usize
        };
        segment(r, len, &mut out);
    }
    out
}

pub fn timestamps(r: &mut Rng, n: usize) -> Vec<i64> {
    let start = if r.chance(4) {
        r.next() as i64
    } else {
        r.range(0, 2_000_000_000_000)
    };
    match r.below(7) {
        0 => {
            let step = r.range(1, 1_000_000);
            (0..n as i64)
                .map(|i| start.wrapping_add(i.wrapping_mul(step)))
                .collect()
        }
        1 => {
            // A scrape interval with jitter and gaps.
            let step = r.range(1, 100_000);
            let jitter = r.range(0, step / 10 + 1);
            let mut t = start;
            (0..n)
                .map(|_| {
                    t = t.wrapping_add(step + r.range(-jitter, jitter));
                    if r.chance(100) {
                        t = t.wrapping_add(step * r.range(2, 1000));
                    }
                    t
                })
                .collect()
        }
        2 => (0..n).map(|_| r.next() as i64).collect(),
        3 => {
            // Backwards steps and duplicates.
            let mut t = start;
            (0..n)
                .map(|_| {
                    if !r.chance(4) {
                        t = t.wrapping_add(r.range(-1000, 1000));
                    }
                    t
                })
                .collect()
        }
        4 => (0..n)
            .map(|_| [i64::MIN, i64::MAX, 0, -1, 1, i64::MIN + 1][r.below(6) as usize])
            .collect(),
        5 => vec![start; n],
        _ => {
            let mut t = start;
            (0..n)
                .map(|_| {
                    t = t.wrapping_add(r.next() as i64 >> r.below(64));
                    t
                })
                .collect()
        }
    }
}

pub fn block_len(r: &mut Rng) -> usize {
    match r.below(6) {
        0 => [1, 2, 3, 7][r.below(4) as usize],
        1 => 64,
        2 => 129,
        3 => tsz_codec::DEFAULT_BLOCK_LEN,
        _ => r.range(1, 300) as usize,
    }
}

pub fn length(r: &mut Rng, block: usize, max: usize) -> usize {
    match r.below(6) {
        0 => r.below(4) as usize,
        1 => block.saturating_sub(1),
        2 => block,
        3 => block + 1,
        4 => 2 * block + 1,
        _ => r.below(max as u64 + 1) as usize,
    }
    .min(max.max(4))
}

pub fn same_bits(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}
