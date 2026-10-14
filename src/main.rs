use std::fmt::Write as _;
use std::io::{Read, Write};
use std::process::ExitCode;

use tsz_codec::{blocks, decode, encode, BlockInfo, Codec, Options, ValueCodec, MAX_BLOCK_LEN};

const USAGE: &str = "\
tsz: lossless time-series compression (Gorilla, Chimp, Chimp128, auto)

usage:
  tsz encode  <in.csv|-> [-o out.tsz] [--codec C] [--block N]   compress, print the size breakdown
  tsz decode  <in.tsz> [-o out.csv] [--bits]                     back to timestamp,value lines
  tsz inspect <in.tsz>                                           blocks, codecs, bits per point
  tsz compare <in.csv>... [--block N]                            bits per point under every codec
  tsz verify  <in.csv>... [--block N]                            round-trip every codec, bit for bit
  tsz version

input: one `timestamp,value` per line. Timestamps are integers (any unit).
Values are anything f64 parses (1.5, -0, NaN, inf) or 0x followed by the
16 hex digits of the exact bit pattern. A header line is skipped.

options:
  --codec C   gorilla, chimp, chimp128 or auto (default auto)
  --block N   points per block (default 4096)
  --bits      print values as 0x bit patterns, so NaN payloads survive text

exit codes: 0 ok, 1 verify found a mismatch, 2 bad input or usage";

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Args, String> {
        const SWITCHES: [&str; 1] = ["--bits"];
        let (mut positional, mut flags) = (Vec::new(), Vec::new());
        let mut it = raw.iter();
        while let Some(a) = it.next() {
            if a.starts_with('-') && a.len() > 1 {
                if SWITCHES.contains(&a.as_str()) {
                    flags.push((a.clone(), None));
                } else {
                    let v = it.next().ok_or(format!("{a} needs a value"))?;
                    flags.push((a.clone(), Some(v.clone())));
                }
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Args { positional, flags })
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn check_known(&self, allowed: &[&str]) -> Result<(), String> {
        for (n, _) in &self.flags {
            if !allowed.contains(&n.as_str()) {
                return Err(format!("unknown option {n}"));
            }
        }
        Ok(())
    }

    fn options(&self) -> Result<Options, String> {
        let mut o = Options::default();
        if let Some(c) = self.get("--codec") {
            o.codec = c.parse()?;
        }
        if let Some(b) = self.get("--block") {
            o.block_len = b
                .parse()
                .ok()
                .filter(|n| (1..=MAX_BLOCK_LEN).contains(n))
                .ok_or(format!("--block must be 1..={MAX_BLOCK_LEN}, got '{b}'"))?;
        }
        Ok(o)
    }

    fn one_input(&self) -> Result<&str, String> {
        match self.positional.as_slice() {
            [p] => Ok(p),
            [] => Err("missing input file".into()),
            _ => Err("expected one input file".into()),
        }
    }
}

fn read_input(path: &str) -> Result<Vec<u8>, String> {
    if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| format!("stdin: {e}"))?;
        Ok(buf)
    } else {
        std::fs::read(path).map_err(|e| format!("{path}: {e}"))
    }
}

struct Csv {
    ts: Vec<i64>,
    vs: Vec<f64>,
}

fn parse_csv(name: &str, data: &[u8]) -> Result<Csv, String> {
    let text = std::str::from_utf8(data).map_err(|_| format!("{name}: not UTF-8 text"))?;
    let mut csv = Csv {
        ts: Vec::new(),
        vs: Vec::new(),
    };
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = |msg: String| format!("{name}:{}: {msg}", i + 1);
        let Some((t, v)) = line.split_once(',') else {
            return Err(at(format!("expected `timestamp,value`, got {line:?}")));
        };
        let (t, v) = (t.trim(), v.trim());
        let Ok(t) = t.parse::<i64>() else {
            if i == 0 {
                continue; // a header
            }
            return Err(at(format!("timestamp {t:?} is not an integer")));
        };
        let v = if let Some(hex) = v.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
                .ok()
                .filter(|_| hex.len() == 16)
                .map(f64::from_bits)
                .ok_or_else(|| at(format!("value {v:?}: 0x needs exactly 16 hex digits")))?
        } else {
            v.parse::<f64>()
                .map_err(|_| at(format!("value {v:?} is not a number")))?
        };
        csv.ts.push(t);
        csv.vs.push(v);
    }
    Ok(csv)
}

fn load_csv(path: &str) -> Result<Csv, String> {
    let name = if path == "-" { "stdin" } else { path };
    parse_csv(name, &read_input(path)?)
}

fn write_output(path: Option<&str>, bytes: &[u8]) -> Result<(), String> {
    match path {
        Some(p) if p != "-" => std::fs::write(p, bytes).map_err(|e| format!("{p}: {e}")),
        _ => std::io::stdout()
            .write_all(bytes)
            .map_err(|e| format!("stdout: {e}")),
    }
}

struct Breakdown {
    points: usize,
    bytes: usize,
    ts: usize,
    values: usize,
}

impl Breakdown {
    fn of(bytes: &[u8], info: &[BlockInfo]) -> Breakdown {
        Breakdown {
            points: info.iter().map(|b| b.points).sum(),
            bytes: bytes.len(),
            ts: info.iter().map(|b| b.ts_bytes).sum(),
            values: info.iter().map(|b| b.value_bytes).sum(),
        }
    }

    fn per_point(&self, bytes: usize) -> f64 {
        if self.points == 0 {
            0.0
        } else {
            bytes as f64 * 8.0 / self.points as f64
        }
    }

    fn lines(&self) -> String {
        let framing = self.bytes - self.ts - self.values;
        let mut s = String::new();
        for (what, b) in [
            ("timestamps", self.ts),
            ("values", self.values),
            ("framing", framing),
        ] {
            let _ = writeln!(
                s,
                "  {what:<11}{:>12} bytes  {:>6.2} bits/point",
                group(b),
                self.per_point(b)
            );
        }
        let _ = write!(
            s,
            "  {:<11}{:>12} bytes  {:>6.2} bits/point  ({:.1}x smaller than 16 bytes/point)",
            "total",
            group(self.bytes),
            self.per_point(self.bytes),
            if self.bytes == 0 {
                0.0
            } else {
                (self.points * 16) as f64 / self.bytes as f64
            }
        );
        s
    }
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn codec_mix<K: Eq + Copy>(
    info: &[BlockInfo],
    key: impl Fn(&BlockInfo) -> K,
    name: impl Fn(K) -> &'static str,
) -> String {
    let mut counts: Vec<(K, usize)> = Vec::new();
    for b in info {
        let k = key(b);
        match counts.iter_mut().find(|(c, _)| *c == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    counts
        .iter()
        .map(|(k, n)| format!("{} {n}", name(*k)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn cmd_encode(a: &Args) -> Result<u8, String> {
    a.check_known(&["-o", "--codec", "--block"])?;
    let input = a.one_input()?;
    let opts = a.options()?;
    let csv = load_csv(input)?;
    let bytes = encode(&csv.ts, &csv.vs, opts);
    let info = blocks(&bytes).expect("own output parses");
    let out = a.get("-o");
    write_output(out, &bytes)?;
    if out.is_some_and(|p| p != "-") {
        println!(
            "{} points, {} blocks of up to {}, codec {}",
            group(csv.ts.len()),
            info.len(),
            opts.block_len,
            opts.codec
        );
        println!("{}", Breakdown::of(&bytes, &info).lines());
    }
    Ok(0)
}

fn cmd_decode(a: &Args) -> Result<u8, String> {
    a.check_known(&["-o", "--bits"])?;
    let input = a.one_input()?;
    let s = decode(&read_input(input)?).map_err(|e| format!("{input}: {e}"))?;
    let mut text = String::with_capacity(s.timestamps.len() * 24);
    for (t, v) in s.timestamps.iter().zip(&s.values) {
        if a.has("--bits") {
            let _ = writeln!(text, "{t},0x{:016x}", v.to_bits());
        } else {
            let _ = writeln!(text, "{t},{v}");
        }
    }
    write_output(a.get("-o"), text.as_bytes())?;
    Ok(0)
}

fn cmd_inspect(a: &Args) -> Result<u8, String> {
    a.check_known(&[])?;
    let input = a.one_input()?;
    let bytes = read_input(input)?;
    let info = blocks(&bytes).map_err(|e| format!("{input}: {e}"))?;
    decode(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let b = Breakdown::of(&bytes, &info);
    println!(
        "{input}: {} points in {} blocks",
        group(b.points),
        info.len()
    );
    println!("{}", b.lines());
    println!(
        "  timestamp codecs  {}",
        codec_mix(&info, |b| b.ts_codec, |c| c.name())
    );
    println!(
        "  value codecs      {}",
        codec_mix(&info, |b| b.value_codec, |c| c.name())
    );
    Ok(0)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn cmd_compare(a: &Args) -> Result<u8, String> {
    a.check_known(&["--block"])?;
    if a.positional.is_empty() {
        return Err("missing input file".into());
    }
    let block_len = a.options()?.block_len;
    let width = a
        .positional
        .iter()
        .map(|p| file_name(p).len())
        .max()
        .unwrap()
        .max(4);
    println!(
        "{:<width$} {:>9}   {:>9} {:>9} {:>9} {:>9}   bits/point; auto's value codecs",
        "file", "points", "gorilla", "chimp", "chimp128", "auto"
    );
    for path in &a.positional {
        let csv = load_csv(path)?;
        let mut row = format!("{:<width$} {:>9}  ", file_name(path), group(csv.ts.len()));
        let mut auto_info = Vec::new();
        for codec in Codec::ALL {
            let bytes = encode(&csv.ts, &csv.vs, Options { codec, block_len });
            let bits = if csv.ts.is_empty() {
                0.0
            } else {
                bytes.len() as f64 * 8.0 / csv.ts.len() as f64
            };
            let _ = write!(row, " {bits:>9.2}");
            if codec == Codec::Auto {
                auto_info = blocks(&bytes).expect("own output parses");
            }
        }
        let mix = codec_mix(&auto_info, |b| b.value_codec, ValueCodec::name);
        println!("{row}   {mix}");
    }
    Ok(0)
}

fn cmd_verify(a: &Args) -> Result<u8, String> {
    a.check_known(&["--block"])?;
    if a.positional.is_empty() {
        return Err("missing input file".into());
    }
    let block_len = a.options()?.block_len;
    let mut bad = 0;
    for path in &a.positional {
        let csv = load_csv(path)?;
        let mut line = format!("{}: {} points", file_name(path), group(csv.ts.len()));
        for codec in Codec::ALL {
            let back = decode(&encode(&csv.ts, &csv.vs, Options { codec, block_len }));
            let ok = back.is_ok_and(|s| {
                s.timestamps == csv.ts
                    && s.values.len() == csv.vs.len()
                    && s.values
                        .iter()
                        .zip(&csv.vs)
                        .all(|(x, y)| x.to_bits() == y.to_bits())
            });
            let _ = write!(line, ", {codec} {}", if ok { "ok" } else { "MISMATCH" });
            bad += !ok as u32;
        }
        println!("{line}");
    }
    Ok(if bad == 0 { 0 } else { 1 })
}

fn run(raw: &[String]) -> Result<u8, String> {
    let Some(cmd) = raw.first() else {
        println!("{USAGE}");
        return Ok(2);
    };
    let a = Args::parse(&raw[1..])?;
    match cmd.as_str() {
        "encode" => cmd_encode(&a),
        "decode" => cmd_decode(&a),
        "inspect" => cmd_inspect(&a),
        "compare" => cmd_compare(&a),
        "verify" => cmd_verify(&a),
        "version" | "--version" => {
            println!("tsz {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(0)
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

/// Rust ignores SIGPIPE, so `tsz decode f | head` would panic on the first
/// write after `head` exits. Restore the default: die quietly, as every
/// other filter in the pipe does.
#[cfg(unix)]
fn restore_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: setting a signal to its default disposition, before any threads exist.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

fn main() -> ExitCode {
    restore_sigpipe();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("tsz: {e}");
            ExitCode::from(2)
        }
    }
}
