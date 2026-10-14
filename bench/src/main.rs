//! Compression ratio and speed of tsz-codec against other crates.
//!
//!     cargo run --release -- [--runs N] [--sizes-only] [--svg ../docs/bench.svg] [--json ../docs/bench.json]
//!
//! Every method must round-trip every dataset bit for bit, or the run fails.
//! Sizes are deterministic; speeds are the median of `--runs` runs on one
//! core of whatever machine runs this.

use std::fmt::Write as _;
use std::hint::black_box;
use std::io::Read;
use std::time::Instant;

use tsz_codec::{Codec, Options};

struct Dataset {
    name: String,
    gz_bytes: usize,
    ts: Vec<i64>,
    vs: Vec<f64>,
}

fn load(dir: &str) -> Vec<Dataset> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".csv.gz"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let gz = std::fs::read(&p).unwrap();
            let mut text = String::new();
            flate2::read::GzDecoder::new(&gz[..])
                .read_to_string(&mut text)
                .unwrap();
            let (mut ts, mut vs) = (Vec::new(), Vec::new());
            for line in text.lines() {
                let (t, v) = line.split_once(',').unwrap();
                ts.push(t.parse().unwrap());
                vs.push(v.parse().unwrap());
            }
            let name = p.file_name().unwrap().to_string_lossy();
            Dataset {
                name: name.trim_end_matches(".csv.gz").to_string(),
                gz_bytes: gz.len(),
                ts,
                vs,
            }
        })
        .collect()
}

type Decoded = (Vec<i64>, Vec<f64>);

/// One way to compress a series.
struct Method {
    name: &'static str,
    /// Shown in the README: what exactly runs.
    detail: &'static str,
    encode: fn(&[i64], &[f64]) -> Vec<u8>,
    decode: fn(&[u8]) -> Decoded,
}

fn ours(codec: Codec) -> impl Fn(&[i64], &[f64]) -> Vec<u8> {
    move |ts, vs| {
        tsz_codec::encode(
            ts,
            vs,
            Options {
                codec,
                ..Options::default()
            },
        )
    }
}

fn ours_decode(b: &[u8]) -> Decoded {
    let s = tsz_codec::decode(b).unwrap();
    (s.timestamps, s.values)
}

fn columns(ts: &[i64], vs: &[f64]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(ts.len() * 16 + 8);
    raw.extend_from_slice(&(ts.len() as u64).to_le_bytes());
    for t in ts {
        raw.extend_from_slice(&t.to_le_bytes());
    }
    for v in vs {
        raw.extend_from_slice(&v.to_le_bytes());
    }
    raw
}

fn uncolumns(raw: &[u8]) -> Decoded {
    let n = u64::from_le_bytes(raw[..8].try_into().unwrap()) as usize;
    let word = |i: usize| raw[8 + 8 * i..16 + 8 * i].try_into().unwrap();
    (
        (0..n).map(|i| i64::from_le_bytes(word(i))).collect(),
        (n..2 * n).map(|i| f64::from_le_bytes(word(i))).collect(),
    )
}

fn methods() -> Vec<Method> {
    vec![
        Method {
            name: "zstd",
            detail:
                "zstd level 3 on the raw columns (all timestamps, then all values, little-endian)",
            encode: |ts, vs| zstd::bulk::compress(&columns(ts, vs), 3).unwrap(),
            decode: |b| uncolumns(&zstd::bulk::decompress(b, 1 << 30).unwrap()),
        },
        Method {
            name: "tsz crate",
            detail: "the `tsz` crate 0.1.4: Gorilla, as Facebook's paper describes it",
            encode: |ts, vs| {
                use gorilla::stream::BufferedWriter;
                use gorilla::{DataPoint, Encode, StdEncoder};
                let mut e = StdEncoder::new(ts[0] as u64, BufferedWriter::new());
                for (&t, &v) in ts.iter().zip(vs) {
                    e.encode(DataPoint::new(t as u64, v));
                }
                e.close().into_vec()
            },
            decode: |b| {
                use gorilla::stream::BufferedReader;
                use gorilla::{Decode, StdDecoder};
                let mut d = StdDecoder::new(BufferedReader::new(b.to_vec().into_boxed_slice()));
                let (mut ts, mut vs) = (Vec::new(), Vec::new());
                while let Ok(p) = d.next() {
                    ts.push(p.get_time() as i64);
                    vs.push(p.get_value());
                }
                (ts, vs)
            },
        },
        Method {
            name: "pco",
            detail: "pco 1.0.4 (pcodec), default level 8, timestamps and values as two chunks",
            encode: |ts, vs| {
                let cfg = pco::ChunkConfig::default();
                let t = pco::standalone::simple_compress(ts, &cfg).unwrap();
                let v = pco::standalone::simple_compress(vs, &cfg).unwrap();
                let mut out = (t.len() as u64).to_le_bytes().to_vec();
                out.extend_from_slice(&t);
                out.extend_from_slice(&v);
                out
            },
            decode: |b| {
                let n = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
                (
                    pco::standalone::simple_decompress(&b[8..8 + n]).unwrap(),
                    pco::standalone::simple_decompress(&b[8 + n..]).unwrap(),
                )
            },
        },
        Method {
            name: "gorilla",
            detail: "this crate, `--codec gorilla`",
            encode: |ts, vs| ours(Codec::Gorilla)(ts, vs),
            decode: ours_decode,
        },
        Method {
            name: "chimp",
            detail: "this crate, `--codec chimp`",
            encode: |ts, vs| ours(Codec::Chimp)(ts, vs),
            decode: ours_decode,
        },
        Method {
            name: "chimp128",
            detail: "this crate, `--codec chimp128`",
            encode: |ts, vs| ours(Codec::Chimp128)(ts, vs),
            decode: ours_decode,
        },
        Method {
            name: "tsz auto",
            detail: "this crate, `--codec auto` (the default)",
            encode: |ts, vs| ours(Codec::Auto)(ts, vs),
            decode: ours_decode,
        },
    ]
}

struct Result {
    dataset: usize,
    method: usize,
    bytes: usize,
    encode_mpts: f64,
    decode_mpts: f64,
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn same(a: &Decoded, ts: &[i64], vs: &[f64]) -> bool {
    a.0 == ts
        && a.1.len() == vs.len()
        && a.1.iter().zip(vs).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .map(|i| args[i + 1].clone())
    };
    let runs: usize = get("--runs").map_or(5, |s| s.parse().unwrap());
    let sizes_only = args.iter().any(|a| a == "--sizes-only");
    let data = load(&get("--data").unwrap_or("../data".into()));
    let methods = methods();

    let mut results = Vec::new();
    for (di, d) in data.iter().enumerate() {
        for (mi, m) in methods.iter().enumerate() {
            let bytes = (m.encode)(&d.ts, &d.vs);
            let back = (m.decode)(&bytes);
            if !same(&back, &d.ts, &d.vs) {
                eprintln!("{} does not round-trip {}", m.name, d.name);
                std::process::exit(1);
            }
            let (mut enc, mut dec) = (Vec::new(), Vec::new());
            for _ in 0..if sizes_only { 0 } else { runs } {
                let t = Instant::now();
                black_box((m.encode)(black_box(&d.ts), black_box(&d.vs)));
                enc.push(d.ts.len() as f64 / t.elapsed().as_secs_f64() / 1e6);
                let t = Instant::now();
                black_box((m.decode)(black_box(&bytes)));
                dec.push(d.ts.len() as f64 / t.elapsed().as_secs_f64() / 1e6);
            }
            results.push(Result {
                dataset: di,
                method: mi,
                bytes: bytes.len(),
                encode_mpts: if sizes_only { 0.0 } else { median(enc) },
                decode_mpts: if sizes_only { 0.0 } else { median(dec) },
            });
        }
    }

    print_tables(&data, &methods, &results, sizes_only);
    if let Some(path) = get("--svg") {
        std::fs::write(&path, svg(&data, &methods, &results)).unwrap();
        eprintln!("wrote {path}");
    }
    if let Some(path) = get("--json") {
        std::fs::write(&path, json(&data, &methods, &results, sizes_only)).unwrap();
        eprintln!("wrote {path}");
    }
}

fn bits(bytes: usize, points: usize) -> f64 {
    bytes as f64 * 8.0 / points as f64
}

fn print_tables(data: &[Dataset], methods: &[Method], results: &[Result], sizes_only: bool) {
    let mut s = String::from("methods\n  csv.gz: the data file itself, gzip -9 of the text\n");
    for m in methods {
        let _ = writeln!(s, "  {}: {}", m.name, m.detail);
    }
    s.push_str("\nbits per point (lower is better)\n\n");
    let _ = write!(s, "| dataset | points | csv.gz |");
    for m in methods {
        let _ = write!(s, " {} |", m.name);
    }
    let _ = write!(s, "\n|---|--:|--:|");
    for _ in methods {
        s.push_str("--:|");
    }
    s.push('\n');
    for (di, d) in data.iter().enumerate() {
        let n = d.ts.len();
        let row: Vec<&Result> = results.iter().filter(|r| r.dataset == di).collect();
        let best = row.iter().map(|r| r.bytes).min().unwrap().min(d.gz_bytes);
        let cell = |bytes: usize| {
            let b = format!("{:.2}", bits(bytes, n));
            if bytes == best {
                format!("**{b}**")
            } else {
                b
            }
        };
        let _ = write!(s, "| {} | {} | {} |", d.name, n, cell(d.gz_bytes));
        for r in &row {
            let _ = write!(s, " {} |", cell(r.bytes));
        }
        s.push('\n');
    }
    if !sizes_only {
        let _ = write!(
            s,
            "\nmillions of points per second, median of runs, one core; encode / decode\n\n| dataset |"
        );
        for m in methods {
            let _ = write!(s, " {} |", m.name);
        }
        let _ = write!(s, "\n|---|");
        for _ in methods {
            s.push_str("--:|");
        }
        s.push('\n');
        for (di, d) in data.iter().enumerate() {
            let _ = write!(s, "| {} |", d.name);
            for r in results.iter().filter(|r| r.dataset == di) {
                let _ = write!(s, " {:.0} / {:.0} |", r.encode_mpts, r.decode_mpts);
            }
            s.push('\n');
        }
    }
    println!("{s}");
}

fn json(data: &[Dataset], methods: &[Method], results: &[Result], sizes_only: bool) -> String {
    let mut s = String::from("{\n  \"note\": \"bytes are deterministic; mpts (millions of points per second) depend on the machine\",\n  \"datasets\": [\n");
    for (di, d) in data.iter().enumerate() {
        let _ = writeln!(
            s,
            "    {{\"name\": \"{}\", \"points\": {}, \"csv_gz_bytes\": {}, \"methods\": [",
            d.name,
            d.ts.len(),
            d.gz_bytes
        );
        let row: Vec<&Result> = results.iter().filter(|r| r.dataset == di).collect();
        for (i, r) in row.iter().enumerate() {
            let _ = write!(
                s,
                "      {{\"method\": \"{}\", \"bytes\": {}, \"bits_per_point\": {:.3}",
                methods[r.method].name,
                r.bytes,
                bits(r.bytes, d.ts.len())
            );
            if !sizes_only {
                let _ = write!(
                    s,
                    ", \"encode_mpts\": {:.1}, \"decode_mpts\": {:.1}",
                    r.encode_mpts, r.decode_mpts
                );
            }
            s.push_str(if i + 1 < row.len() { "},\n" } else { "}\n" });
        }
        s.push_str(if di + 1 < data.len() {
            "    ]},\n"
        } else {
            "    ]}\n"
        });
    }
    s.push_str("  ]\n}\n");
    s
}

const INK: &str = "#0b0b0b";
const MUTED: &str = "#52514e";
const GRID: &str = "#e4e3df";
const BAR: &str = "#b9b8b2";
const ACCENT: &str = "#2a78d6";
const FONT: &str = "-apple-system, 'Segoe UI', Helvetica, Arial, sans-serif";

/// Small multiples: one panel per dataset, one bar per method, all on the
/// same 0-64 bits/point scale. Labels carry identity, so one neutral fill
/// plus an accent for tsz auto is enough colour.
fn svg(data: &[Dataset], methods: &[Method], results: &[Result]) -> String {
    let shown: Vec<usize> = ["tsz crate", "chimp128", "zstd", "pco", "tsz auto"]
        .iter()
        .map(|n| methods.iter().position(|m| m.name == *n).unwrap())
        .collect();
    let rows = shown.len() + 1; // + csv.gz
    let (cols, panel_w, label_w, bar_h, gap) = (2usize, 470.0, 92.0, 16.0, 6.0);
    let panel_h = 34.0 + rows as f64 * (bar_h + gap) + 8.0;
    let (left, top) = (24.0, 78.0);
    let width = left * 2.0 + cols as f64 * panel_w;
    let panel_rows = data.len().div_ceil(cols);
    let height = top + panel_rows as f64 * panel_h + 40.0;
    let scale_max = 72.0;
    let plot_w = panel_w - label_w - 70.0;

    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{FONT}">"#
    );
    let _ = writeln!(s, r##"<rect width="100%" height="100%" fill="#fcfcfb"/>"##);
    let _ = writeln!(
        s,
        r#"<text x="{left}" y="30" font-size="17" font-weight="600" fill="{INK}">Bits per point on six real series (lower is better)</text>"#
    );
    let _ = writeln!(
        s,
        r#"<text x="{left}" y="51" font-size="12.5" fill="{MUTED}">Timestamp and value together; uncompressed is 128. All lossless. “tsz crate” is the existing Gorilla crate; “tsz auto” is this one.</text>"#
    );

    for (di, d) in data.iter().enumerate() {
        let (px, py) = (
            left + (di % cols) as f64 * panel_w,
            top + (di / cols) as f64 * panel_h,
        );
        let n = d.ts.len();
        let _ = writeln!(
            s,
            r#"<text x="{px:.1}" y="{:.1}" font-size="13" font-weight="600" fill="{INK}">{}</text>"#,
            py + 14.0,
            d.name
        );
        let _ = writeln!(
            s,
            r#"<text x="{:.1}" y="{:.1}" font-size="11" fill="{MUTED}">{} points</text>"#,
            px + 8.0 * d.name.len() as f64 + 6.0,
            py + 14.0,
            group(n)
        );
        let x0 = px + label_w;
        let by0 = py + 26.0;
        for tick in [0.0, 16.0, 32.0, 48.0, 64.0] {
            let x = x0 + tick / scale_max * plot_w;
            let _ = writeln!(
                s,
                r#"<line x1="{x:.1}" y1="{by0:.1}" x2="{x:.1}" y2="{:.1}" stroke="{GRID}"/>"#,
                by0 + rows as f64 * (bar_h + gap) - gap
            );
            if di / cols == panel_rows - 1 || di + cols >= data.len() {
                let _ = writeln!(
                    s,
                    r#"<text x="{x:.1}" y="{:.1}" font-size="10.5" text-anchor="middle" fill="{MUTED}">{tick:.0}</text>"#,
                    by0 + rows as f64 * (bar_h + gap) + 8.0
                );
            }
        }
        let mut bars: Vec<(&str, usize, bool)> = vec![("csv.gz", d.gz_bytes, false)];
        for &mi in &shown {
            let r = results
                .iter()
                .find(|r| r.dataset == di && r.method == mi)
                .unwrap();
            bars.push((methods[mi].name, r.bytes, methods[mi].name == "tsz auto"));
        }
        for (i, (name, bytes, hi)) in bars.iter().enumerate() {
            let y = by0 + i as f64 * (bar_h + gap);
            let b = bits(*bytes, n);
            let w = (b.min(scale_max) / scale_max * plot_w).max(1.0);
            let fill = if *hi { ACCENT } else { BAR };
            let weight = if *hi { "600" } else { "400" };
            let _ = writeln!(
                s,
                r#"<text x="{:.1}" y="{:.1}" font-size="11.5" text-anchor="end" font-weight="{weight}" fill="{INK}">{name}</text>"#,
                x0 - 8.0,
                y + bar_h - 4.0
            );
            // Square at the baseline, rounded at the data end.
            let _ = writeln!(
                s,
                r#"<path d="M{x0:.1},{y:.1} h{:.1} a4,4 0 0 1 4,4 v{:.1} a4,4 0 0 1 -4,4 h{:.1} z" fill="{fill}"><title>{name}: {b:.2} bits/point</title></path>"#,
                (w - 4.0).max(0.0),
                bar_h - 8.0,
                -(w - 4.0).max(0.0)
            );
            let _ = writeln!(
                s,
                r#"<text x="{:.1}" y="{:.1}" font-size="11" font-weight="{weight}" fill="{INK}">{b:.1}</text>"#,
                x0 + w + 6.0,
                y + bar_h - 4.0
            );
        }
    }
    let _ = writeln!(
        s,
        r#"<text x="{left}" y="{:.1}" font-size="11" fill="{MUTED}">Sources: NOAA CO-OPS station 9414290 (San Francisco), 2024; UCI household power consumption, 2007; Wikimedia pageviews, 2024. See data/README.md.</text>"#,
        height - 12.0
    );
    s.push_str("</svg>\n");
    s
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
