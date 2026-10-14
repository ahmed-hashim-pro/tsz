//! Decoding untrusted bytes must fail cleanly, never panic or allocate wildly.

mod common;

use common::*;
use tsz_codec::{decode, encode, Codec, Error, Options};

fn sample(seed: u64) -> Vec<u8> {
    let r = &mut Rng::new(seed);
    let n = r.range(0, 400) as usize;
    let codec = Codec::ALL[r.below(4) as usize];
    let block_len = block_len(r);
    encode(
        &timestamps(r, n),
        &values(r, n),
        Options { codec, block_len },
    )
}

#[test]
fn every_truncation_is_an_error() {
    for seed in 0..300 {
        let bytes = sample(seed);
        for cut in 0..bytes.len() {
            assert!(
                decode(&bytes[..cut]).is_err(),
                "seed {seed}: prefix of {cut} bytes decoded"
            );
        }
    }
}

#[test]
fn flipped_bits_never_panic() {
    for seed in 0..2000 {
        let mut bytes = sample(seed);
        let r = &mut Rng::new(seed ^ 0xabcdef);
        for _ in 0..1 + r.below(4) {
            let i = r.below(bytes.len() as u64) as usize;
            bytes[i] ^= 1 << r.below(8);
        }
        let _ = decode(&bytes);
    }
}

#[test]
fn random_bytes_after_the_magic_never_panic() {
    let r = &mut Rng::new(99);
    for _ in 0..20_000 {
        let mut bytes = b"TSZ\x01".to_vec();
        let n = r.below(200);
        bytes.extend((0..n).map(|_| r.next() as u8));
        let _ = decode(&bytes);
    }
}

#[test]
fn framing_errors_name_the_problem() {
    let good = encode(&[1, 2, 3], &[1.0, 2.0, 3.0], Options::default());
    assert_eq!(decode(b"GZIP"), Err(Error::BadMagic));
    assert_eq!(decode(b"TSZ\x02\x00"), Err(Error::UnsupportedVersion(2)));
    assert_eq!(decode(b"TS"), Err(Error::Truncated));
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(matches!(decode(&trailing), Err(Error::Corrupt(_))));
    let mut codec = good.clone();
    codec[5] = 0x0f; // value codec 15
    assert_eq!(decode(&codec), Err(Error::UnknownCodec(0x0f)));
    // A header claiming a huge block is refused before anything is allocated.
    assert!(matches!(
        decode(b"TSZ\x01\xff\xff\xff\xff\x0f\x00\x00\x00\x00"),
        Err(Error::Corrupt(_))
    ));
}
