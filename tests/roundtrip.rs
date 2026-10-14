//! Every codec must give back exactly what it was given: every timestamp,
//! and every value bit for bit.

mod common;

use common::*;
use tsz_codec::{blocks, decode, encode, Codec, Encoder, Options, ValueCodec};

fn check(seed: u64, ts: &[i64], vs: &[f64], opts: Options) -> usize {
    let bytes = encode(ts, vs, opts);
    let back =
        decode(&bytes).unwrap_or_else(|e| panic!("seed {seed} {opts:?}: decode failed: {e}"));
    assert_eq!(
        back.timestamps, ts,
        "seed {seed} {opts:?}: timestamps differ"
    );
    assert!(
        same_bits(&back.values, vs),
        "seed {seed} {opts:?}: values differ"
    );
    let info = blocks(&bytes).unwrap();
    assert_eq!(info.iter().map(|b| b.points).sum::<usize>(), ts.len());
    assert!(info.iter().all(|b| b.points <= opts.block_len));
    bytes.len()
}

fn run(seeds: std::ops::Range<u64>, max_len: usize) {
    for seed in seeds {
        let r = &mut Rng::new(seed);
        let block_len = block_len(r);
        let n = length(r, block_len, max_len);
        let ts = timestamps(r, n);
        let vs = values(r, n);
        let mut sizes = [0; 4];
        for (i, codec) in Codec::ALL.into_iter().enumerate() {
            sizes[i] = check(seed, &ts, &vs, Options { codec, block_len });
        }
        // Auto chooses per block from a superset of what the others use.
        let fixed = sizes[..3].iter().min().unwrap();
        assert!(
            sizes[3] <= *fixed,
            "seed {seed}: auto {} > best fixed {fixed}",
            sizes[3]
        );
    }
}

#[test]
fn random_series_round_trip_under_every_codec() {
    let n = std::env::var("TSZ_SERIES").map_or(20_000, |s| s.parse().unwrap());
    run(0..n, 3000);
}

/// The long run behind the README's claim. `make soak` runs it.
#[test]
#[ignore]
fn soak() {
    let n: u64 = std::env::var("TSZ_SOAK").map_or(1_000_000, |s| s.parse().unwrap());
    let base = 1 << 32;
    run(base..base + n, 600);
}

#[test]
fn edge_lengths_and_block_boundaries() {
    let r = &mut Rng::new(1);
    for block_len in [1, 2, 3, 1024] {
        for n in [
            0,
            1,
            2,
            3,
            block_len - 1,
            block_len,
            block_len + 1,
            3 * block_len,
        ] {
            let ts = timestamps(r, n);
            let vs = values(r, n);
            for codec in Codec::ALL {
                check(0, &ts, &vs, Options { codec, block_len });
            }
        }
    }
}

#[test]
fn every_special_value_next_to_every_other() {
    let specials: Vec<f64> = [
        0x7ff8_0000_0000_0000u64,
        0x7ff8_dead_beef_0001,
        0xfff8_0000_0000_0000,
        0x7ff0_0000_0000_0001, // signalling NaN
        0xfff7_ffff_ffff_ffff,
        0,
        1 << 63,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        1,
        0x000f_ffff_ffff_ffff,
        f64::MAX.to_bits(),
        f64::MIN.to_bits(),
        1.0f64.to_bits(),
    ]
    .into_iter()
    .map(f64::from_bits)
    .collect();
    let mut vs = Vec::new();
    for &a in &specials {
        for &b in &specials {
            vs.extend([a, b, b, a]);
        }
    }
    let ts: Vec<i64> = (0..vs.len() as i64)
        .map(|i| [i64::MIN, i64::MAX, 0, -1][i as usize % 4] ^ i)
        .collect();
    for codec in Codec::ALL {
        for block_len in [1, 5, 64, 4096] {
            check(0, &ts, &vs, Options { codec, block_len });
        }
    }
}

#[test]
fn streaming_encoder_matches_encode() {
    let r = &mut Rng::new(7);
    let (ts, vs) = (timestamps(r, 5000), values(r, 5000));
    for codec in Codec::ALL {
        let opts = Options {
            codec,
            block_len: 333,
        };
        let mut e = Encoder::new(opts);
        for (&t, &v) in ts.iter().zip(&vs) {
            e.push(t, v);
        }
        assert_eq!(e.finish(), encode(&ts, &vs, opts));
    }
}

#[test]
fn printed_decimals_take_the_decimal_codec() {
    let vs: Vec<f64> = (0..4096)
        .map(|i| {
            format!("{:.3}", 1.0 + (i as f64 * 0.01).sin())
                .parse()
                .unwrap()
        })
        .collect();
    let ts: Vec<i64> = (0..4096).map(|i| 1_700_000_000 + 360 * i).collect();
    let bytes = encode(&ts, &vs, Options::default());
    let info = blocks(&bytes).unwrap();
    assert!(
        info.iter().all(|b| b.value_codec == ValueCodec::Decimal),
        "{info:?}"
    );
    let gorilla = encode(
        &ts,
        &vs,
        Options {
            codec: Codec::Gorilla,
            ..Options::default()
        },
    );
    assert!(
        bytes.len() * 3 < gorilla.len(),
        "auto {} vs gorilla {}",
        bytes.len(),
        gorilla.len()
    );
    // One -0.0 can't be n / 10^d, so its block falls back to a float codec.
    let mut vs2 = vs.clone();
    vs2[10] = -0.0;
    let info = blocks(&encode(&ts, &vs2, Options::default())).unwrap();
    assert_ne!(info[0].value_codec, ValueCodec::Decimal);
    assert!(info[1..]
        .iter()
        .all(|b| b.value_codec == ValueCodec::Decimal));
    check(0, &ts, &vs2, Options::default());
}

/// Pins the bit costs the papers describe, so a format change can't slip in.
#[test]
fn bit_costs_match_the_papers() {
    let n = 1001usize;
    let ts: Vec<i64> = (0..n as i64).map(|i| 1000 + 60 * i).collect();
    let constant = vec![42.5f64; n];
    let payload = |codec, vs: &[f64]| {
        let b = blocks(&encode(
            &ts,
            vs,
            Options {
                codec,
                block_len: n,
            },
        ))
        .unwrap();
        (b[0].ts_bytes, b[0].value_bytes)
    };
    // Timestamps: 64 raw bits, the first delta (60 -> zigzag 120, 7-bit bucket: 9 bits), then 999 zero bits.
    let ts_bytes = (64 + 9 + 999usize).div_ceil(8);
    // Constant values: 64 raw bits, then per value 1 bit (Gorilla), 2 (Chimp), 9 (Chimp128).
    assert_eq!(
        payload(Codec::Gorilla, &constant),
        (ts_bytes, (64 + 1000usize).div_ceil(8))
    );
    assert_eq!(
        payload(Codec::Chimp, &constant),
        (ts_bytes, (64 + 2 * 1000usize).div_ceil(8))
    );
    assert_eq!(
        payload(Codec::Chimp128, &constant),
        (ts_bytes, (64 + 9 * 1000usize).div_ceil(8))
    );
}

/// Files outlive the code that wrote them. These hashes pin the exact bytes
/// each codec writes for one fixed series; if one changes, the format did.
#[test]
fn format_is_stable() {
    let r = &mut Rng::new(2024);
    let ts = timestamps(r, 20_000);
    let vs = values(r, 20_000);
    let fnv = |b: &[u8]| {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
            (h ^ x as u64).wrapping_mul(0x100_0000_01b3)
        })
    };
    let got: Vec<(usize, u64)> = Codec::ALL
        .into_iter()
        .map(|codec| {
            let b = encode(
                &ts,
                &vs,
                Options {
                    codec,
                    block_len: 512,
                },
            );
            (b.len(), fnv(&b))
        })
        .collect();
    let printed = format!("{got:?}");
    assert_eq!(
        printed,
        "[(172696, 5850786183253521888), (145079, 7027276607697672976), (121453, 13122321198706284139), (110176, 14040927783670235217)]",
        "update the pinned value only for a deliberate format change");
}
