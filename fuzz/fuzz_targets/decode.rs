//! Arbitrary bytes, with and without the magic in front: decoding must
//! return Ok or Err, never panic, hang or allocate without bound.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tsz_codec::decode(data);
    let mut framed = b"TSZ\x01".to_vec();
    framed.extend_from_slice(data);
    if let Ok(s) = tsz_codec::decode(&framed) {
        // Anything that decodes must have consistent framing.
        let points: usize = tsz_codec::blocks(&framed)
            .unwrap()
            .iter()
            .map(|b| b.points)
            .sum();
        assert_eq!(points, s.timestamps.len());
        assert_eq!(points, s.values.len());
    }
});
