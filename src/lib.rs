//! Lossless compression for `(i64 timestamp, f64 value)` series: Gorilla,
//! Chimp and Chimp128, plus an `Auto` mode that picks the smallest encoding
//! for each block, including one for values that were printed as decimals.
//!
//! ```
//! use tsz_codec::{decode, encode, Codec, Options};
//!
//! let ts: Vec<i64> = (0..1000).map(|i| 1_700_000_000 + 60 * i).collect();
//! let vs: Vec<f64> = (0..1000).map(|i| (i % 37) as f64 / 10.0).collect();
//! let bytes = encode(&ts, &vs, Options { codec: Codec::Auto, ..Options::default() });
//! let back = decode(&bytes).unwrap();
//! assert_eq!(back.timestamps, ts);
//! assert!(back.values.iter().zip(&vs).all(|(a, b)| a.to_bits() == b.to_bits()));
//! ```
//!
//! Every i64 timestamp sequence (unsorted, duplicated, wrapping) and every
//! f64 bit pattern (NaN payloads, -0.0, infinities, subnormals) round-trips
//! bit for bit, under every codec. Decoding never panics: malformed input is
//! an [`Error`].
//!
//! # Format
//!
//! ```text
//! "TSZ" 0x01
//! block*            varint points (1..=MAX_BLOCK_LEN)
//!                   u8     timestamp codec << 4 | value codec
//!                   varint timestamp bytes, then those bytes
//!                   varint value bytes, then those bytes
//! 0x00              end of stream; nothing may follow
//! ```
//!
//! Blocks are independent: each starts from a raw first timestamp and value.

mod bits;
mod chimp;
mod decimal;
mod gorilla;
mod intcode;

use bits::{BitCounter, BitReader, BitWriter, Sink};
use std::fmt;

const MAGIC: [u8; 4] = *b"TSZ\x01";

/// Points per block unless [`Options::block_len`] says otherwise.
pub const DEFAULT_BLOCK_LEN: usize = 4096;

/// The largest block a decoder accepts, and so the largest `block_len`.
pub const MAX_BLOCK_LEN: usize = 1 << 20;

/// What the encoder may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Gorilla delta-of-delta timestamps, Gorilla XOR values.
    Gorilla,
    /// Gorilla timestamps, Chimp values.
    Chimp,
    /// Gorilla timestamps, Chimp128 values.
    Chimp128,
    /// For each block, the smallest of: Gorilla or adaptive timestamps, and
    /// Gorilla, Chimp, Chimp128 or decimal values.
    Auto,
}

impl Codec {
    pub const ALL: [Codec; 4] = [Codec::Gorilla, Codec::Chimp, Codec::Chimp128, Codec::Auto];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Gorilla => "gorilla",
            Codec::Chimp => "chimp",
            Codec::Chimp128 => "chimp128",
            Codec::Auto => "auto",
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Codec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Codec::ALL
            .into_iter()
            .find(|c| c.name() == s)
            .ok_or_else(|| {
                format!("unknown codec {s:?} (expected gorilla, chimp, chimp128 or auto)")
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub codec: Codec,
    /// Points per block, 1..=[`MAX_BLOCK_LEN`].
    pub block_len: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            codec: Codec::Auto,
            block_len: DEFAULT_BLOCK_LEN,
        }
    }
}

/// How a block's timestamps are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TsCodec {
    /// Gorilla's delta-of-delta buckets.
    DeltaOfDelta = 0,
    /// First or second differences with bucket widths fitted to the block.
    Adaptive = 1,
}

/// How a block's values are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueCodec {
    Gorilla = 0,
    Chimp = 1,
    Chimp128 = 2,
    /// Integers `n` with every value exactly `n / 10^d`.
    Decimal = 3,
}

impl TsCodec {
    fn from_u8(b: u8) -> Option<Self> {
        [TsCodec::DeltaOfDelta, TsCodec::Adaptive]
            .get(b as usize)
            .copied()
    }
    pub fn name(self) -> &'static str {
        match self {
            TsCodec::DeltaOfDelta => "delta-of-delta",
            TsCodec::Adaptive => "adaptive",
        }
    }
}

impl ValueCodec {
    fn from_u8(b: u8) -> Option<Self> {
        [
            ValueCodec::Gorilla,
            ValueCodec::Chimp,
            ValueCodec::Chimp128,
            ValueCodec::Decimal,
        ]
        .get(b as usize)
        .copied()
    }
    pub fn name(self) -> &'static str {
        match self {
            ValueCodec::Gorilla => "gorilla",
            ValueCodec::Chimp => "chimp",
            ValueCodec::Chimp128 => "chimp128",
            ValueCodec::Decimal => "decimal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input doesn't start with the tsz magic bytes.
    BadMagic,
    /// A format version this build can't read.
    UnsupportedVersion(u8),
    /// The input ends in the middle of something.
    Truncated,
    /// A block names a codec this build doesn't know.
    UnknownCodec(u8),
    /// The bytes can't have come from the encoder.
    Corrupt(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::BadMagic => f.write_str("not a tsz stream (bad magic bytes)"),
            Error::UnsupportedVersion(v) => write!(f, "tsz format version {v} is not supported"),
            Error::Truncated => f.write_str("truncated tsz stream"),
            Error::UnknownCodec(c) => write!(f, "unknown codec byte 0x{c:02x}"),
            Error::Corrupt(why) => write!(f, "corrupt tsz stream: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Scratch space reused from block to block.
#[derive(Default)]
struct Work {
    bits: Vec<u64>,
    ints: Vec<i64>,
    residuals: Vec<u64>,
    table: chimp::Chimp128Table,
}

/// Encodes points one at a time, a block at a time.
pub struct Encoder {
    opts: Options,
    out: Vec<u8>,
    ts: Vec<i64>,
    vs: Vec<f64>,
    work: Work,
}

impl Encoder {
    /// # Panics
    /// If `opts.block_len` is 0 or more than [`MAX_BLOCK_LEN`].
    pub fn new(opts: Options) -> Self {
        assert!(
            (1..=MAX_BLOCK_LEN).contains(&opts.block_len),
            "block_len must be 1..={MAX_BLOCK_LEN}, got {}",
            opts.block_len
        );
        Self {
            opts,
            out: MAGIC.to_vec(),
            ts: Vec::with_capacity(opts.block_len),
            vs: Vec::with_capacity(opts.block_len),
            work: Work::default(),
        }
    }

    pub fn push(&mut self, t: i64, v: f64) {
        self.ts.push(t);
        self.vs.push(v);
        if self.ts.len() == self.opts.block_len {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.ts.is_empty() {
            encode_block(
                &self.ts,
                &self.vs,
                self.opts.codec,
                &mut self.work,
                &mut self.out,
            );
            self.ts.clear();
            self.vs.clear();
        }
    }

    /// Writes the last block and the end marker.
    pub fn finish(mut self) -> Vec<u8> {
        self.flush();
        self.out.push(0);
        self.out
    }
}

/// Encodes a whole series.
///
/// # Panics
/// If the slices differ in length, or `opts.block_len` is out of range.
pub fn encode(timestamps: &[i64], values: &[f64], opts: Options) -> Vec<u8> {
    assert_eq!(
        timestamps.len(),
        values.len(),
        "timestamps and values differ in length"
    );
    let mut enc = Encoder::new(opts);
    enc.out.reserve(timestamps.len() * 2);
    for (ts, vs) in timestamps
        .chunks(opts.block_len)
        .zip(values.chunks(opts.block_len))
    {
        encode_block(ts, vs, opts.codec, &mut enc.work, &mut enc.out);
    }
    enc.finish()
}

fn encode_ts(ts: &[i64], tc: TsCodec, w: &mut impl Sink, work: &mut Work) {
    match tc {
        TsCodec::DeltaOfDelta => gorilla::encode_ts(ts, w),
        TsCodec::Adaptive => intcode::encode(ts, w, &mut work.residuals),
    }
}

fn encode_values(vc: ValueCodec, digits: u32, w: &mut impl Sink, work: &mut Work) {
    match vc {
        ValueCodec::Gorilla => gorilla::encode_values(&work.bits, w),
        ValueCodec::Chimp => chimp::encode(&work.bits, w),
        ValueCodec::Chimp128 => chimp::encode128(&work.bits, w, &mut work.table),
        ValueCodec::Decimal => decimal::encode(digits, &work.ints, w, &mut work.residuals),
    }
}

// The cheapest of `candidates` under `cost`, which gets a counter that stops
// once it reaches the best so far. Ties go to the earlier candidate.
fn cheapest<C: Copy>(
    candidates: &[C],
    work: &mut Work,
    mut cost: impl FnMut(C, &mut BitCounter, &mut Work),
) -> C {
    let mut best = (candidates[0], u64::MAX);
    for &c in candidates {
        let mut counter = BitCounter::with_limit(best.1);
        cost(c, &mut counter, work);
        if counter.len() < best.1 {
            best = (c, counter.len());
        }
    }
    best.0
}

fn encode_block(ts: &[i64], vs: &[f64], codec: Codec, work: &mut Work, out: &mut Vec<u8>) {
    work.bits.clear();
    work.bits.extend(vs.iter().map(|v| v.to_bits()));
    let (ts_codec, value_codec, digits) = match codec {
        Codec::Gorilla => (TsCodec::DeltaOfDelta, ValueCodec::Gorilla, 0),
        Codec::Chimp => (TsCodec::DeltaOfDelta, ValueCodec::Chimp, 0),
        Codec::Chimp128 => (TsCodec::DeltaOfDelta, ValueCodec::Chimp128, 0),
        Codec::Auto => {
            let tc = cheapest(
                &[TsCodec::DeltaOfDelta, TsCodec::Adaptive],
                work,
                |tc, c, w| encode_ts(ts, tc, c, w),
            );
            // Decimal goes first: when it applies it is usually far smaller,
            // and its size becomes the budget the float codecs must beat.
            let digits = decimal::digits(vs, &mut work.ints);
            let all = [
                ValueCodec::Decimal,
                ValueCodec::Chimp128,
                ValueCodec::Chimp,
                ValueCodec::Gorilla,
            ];
            let candidates = if digits.is_some() {
                &all[..]
            } else {
                &all[1..]
            };
            let d = digits.unwrap_or(0);
            let vc = cheapest(candidates, work, |vc, c, w| encode_values(vc, d, c, w));
            (tc, vc, d)
        }
    };
    let mut tw = BitWriter::new();
    encode_ts(ts, ts_codec, &mut tw, work);
    let mut vw = BitWriter::new();
    encode_values(value_codec, digits, &mut vw, work);

    let tbytes = tw.finish();
    let vbytes = vw.finish();
    put_varint(out, ts.len() as u64);
    out.push((ts_codec as u8) << 4 | value_codec as u8);
    put_varint(out, tbytes.len() as u64);
    out.extend_from_slice(&tbytes);
    put_varint(out, vbytes.len() as u64);
    out.extend_from_slice(&vbytes);
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn byte(&mut self) -> Result<u8, Error> {
        let b = *self.data.get(self.pos).ok_or(Error::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            let low = (b & 0x7f) as u64;
            if shift == 63 && low > 1 {
                return Err(Error::Corrupt("varint overflows 64 bits"));
            }
            v |= low << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error::Corrupt("varint longer than 10 bytes"))
    }

    fn bytes(&mut self, n: u64) -> Result<&'a [u8], Error> {
        if n > (self.data.len() - self.pos) as u64 {
            return Err(Error::Truncated);
        }
        let s = &self.data[self.pos..self.pos + n as usize];
        self.pos += n as usize;
        Ok(s)
    }
}

/// One block's framing, as [`blocks`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    pub points: usize,
    pub ts_codec: TsCodec,
    pub value_codec: ValueCodec,
    /// Bytes of timestamp payload.
    pub ts_bytes: usize,
    /// Bytes of value payload.
    pub value_bytes: usize,
    /// Bytes of framing: point count, codec byte, two lengths.
    pub header_bytes: usize,
}

struct RawBlock<'a> {
    info: BlockInfo,
    ts: &'a [u8],
    vs: &'a [u8],
}

fn walk(bytes: &[u8]) -> Result<Vec<RawBlock<'_>>, Error> {
    if bytes.len() < 4 || bytes[..3] != MAGIC[..3] {
        return Err(if bytes.len() < 4 && MAGIC.starts_with(bytes) {
            Error::Truncated
        } else {
            Error::BadMagic
        });
    }
    if bytes[3] != MAGIC[3] {
        return Err(Error::UnsupportedVersion(bytes[3]));
    }
    let mut c = Cursor {
        data: bytes,
        pos: 4,
    };
    let mut out = Vec::new();
    loop {
        let start = c.pos;
        let n = c.varint()?;
        if n == 0 {
            break;
        }
        if n > MAX_BLOCK_LEN as u64 {
            return Err(Error::Corrupt("block longer than MAX_BLOCK_LEN"));
        }
        let codecs = c.byte()?;
        let ts_codec = TsCodec::from_u8(codecs >> 4).ok_or(Error::UnknownCodec(codecs))?;
        let value_codec = ValueCodec::from_u8(codecs & 15).ok_or(Error::UnknownCodec(codecs))?;
        let tlen = c.varint()?;
        let ts = c.bytes(tlen)?;
        let vlen = c.varint()?;
        let vs = c.bytes(vlen)?;
        out.push(RawBlock {
            info: BlockInfo {
                points: n as usize,
                ts_codec,
                value_codec,
                ts_bytes: ts.len(),
                value_bytes: vs.len(),
                header_bytes: c.pos - start - ts.len() - vs.len(),
            },
            ts,
            vs,
        });
    }
    if c.pos != bytes.len() {
        return Err(Error::Corrupt("bytes after the end marker"));
    }
    Ok(out)
}

/// The framing of every block, without decoding the payloads.
pub fn blocks(bytes: &[u8]) -> Result<Vec<BlockInfo>, Error> {
    Ok(walk(bytes)?.into_iter().map(|b| b.info).collect())
}

/// A decoded series.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub timestamps: Vec<i64>,
    pub values: Vec<f64>,
}

/// Decodes a stream from [`encode`] or [`Encoder`].
pub fn decode(bytes: &[u8]) -> Result<Series, Error> {
    let raw = walk(bytes)?;
    let total: usize = raw.iter().map(|b| b.info.points).sum();
    // Every point costs at least a bit, so this can't be inflated by a lying header.
    let cap = total.min(bytes.len() * 8);
    let mut s = Series {
        timestamps: Vec::with_capacity(cap),
        values: Vec::with_capacity(cap),
    };
    let mut ints = Vec::new();
    for b in raw {
        let n = b.info.points;
        let mut r = BitReader::new(b.ts);
        match b.info.ts_codec {
            TsCodec::DeltaOfDelta => gorilla::decode_ts(&mut r, n, &mut s.timestamps)?,
            TsCodec::Adaptive => intcode::decode(&mut r, n, &mut s.timestamps)?,
        }
        exact(&r, b.ts)?;
        let mut r = BitReader::new(b.vs);
        match b.info.value_codec {
            ValueCodec::Gorilla => gorilla::decode_values(&mut r, n, &mut s.values)?,
            ValueCodec::Chimp => chimp::decode(&mut r, n, &mut s.values)?,
            ValueCodec::Chimp128 => chimp::decode128(&mut r, n, &mut s.values)?,
            ValueCodec::Decimal => decimal::decode(&mut r, n, &mut s.values, &mut ints)?,
        }
        exact(&r, b.vs)?;
    }
    Ok(s)
}

fn exact(r: &BitReader, payload: &[u8]) -> Result<(), Error> {
    if r.bytes_used() == payload.len() {
        Ok(())
    } else {
        Err(Error::Corrupt("block payload longer than its points"))
    }
}
