//! Arbitrary series under arbitrary options: every codec must give back
//! every timestamp and every value bit for bit.
#![no_main]
use libfuzzer_sys::fuzz_target;
use tsz_codec::{decode, encode, Codec, Options};

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let codec = Codec::ALL[data[0] as usize % 4];
    let block_len = 1 + data[1] as usize % 300;
    let (mut ts, mut vs) = (Vec::new(), Vec::new());
    for c in data[2..].chunks_exact(16) {
        ts.push(i64::from_le_bytes(c[..8].try_into().unwrap()));
        vs.push(f64::from_bits(u64::from_le_bytes(
            c[8..].try_into().unwrap(),
        )));
    }
    let bytes = encode(&ts, &vs, Options { codec, block_len });
    let back = decode(&bytes).expect("own output decodes");
    assert_eq!(back.timestamps, ts);
    assert!(back
        .values
        .iter()
        .zip(&vs)
        .all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(back.values.len(), vs.len());
});
