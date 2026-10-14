//! The command line, run as a user would run it.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn tsz(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsz"))
        .args(args)
        .output()
        .unwrap()
}

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tsz-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().into_owned()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const CSV: &str = "timestamp,value
1700000000,1.327
1700000360,1.297
1700000720,-0
1700001080,NaN
1700001440,inf
1700001800,0x7ff8deadbeef0001
1700001800,1e-310
";

#[test]
fn encode_then_decode_gives_back_the_exact_bits() {
    let (csv, tsz_file, out) = (tmp("a.csv"), tmp("a.tsz"), tmp("a.out.csv"));
    std::fs::write(&csv, CSV).unwrap();
    let o = tsz(&["encode", &csv, "-o", &tsz_file]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("7 points"), "{}", text(&o.stdout));
    let o = tsz(&["decode", &tsz_file, "--bits", "-o", &out]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let lines: Vec<String> = std::fs::read_to_string(&out)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 7);
    assert_eq!(
        lines[0],
        format!("1700000000,0x{:016x}", 1.327f64.to_bits())
    );
    assert_eq!(lines[2], "1700000720,0x8000000000000000"); // -0.0 kept its sign
    assert_eq!(lines[5], "1700001800,0x7ff8deadbeef0001"); // NaN kept its payload
                                                           // Without --bits, values print in shortest round-trip form.
    let o = tsz(&["decode", &tsz_file]);
    assert!(text(&o.stdout).starts_with(
        "1700000000,1.327\n1700000360,1.297\n1700000720,-0\n1700001080,NaN\n1700001440,inf\n"
    ));
}

#[test]
fn reads_stdin_and_writes_stdout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tsz"))
        .args(["encode", "-", "--codec", "chimp128"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(CSV.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    assert_eq!(&o.stdout[..4], b"TSZ\x01");
}

#[test]
fn bad_input_names_the_line_and_exits_2() {
    let csv = tmp("bad.csv");
    std::fs::write(&csv, "1,2\n3,4\n5,four\n").unwrap();
    let o = tsz(&["encode", &csv, "-o", &tmp("bad.tsz")]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        text(&o.stderr).contains("bad.csv:3: value \"four\" is not a number"),
        "{}",
        text(&o.stderr)
    );

    std::fs::write(&csv, "1,2\nx,4\n").unwrap();
    let o = tsz(&["encode", &csv]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("bad.csv:2: timestamp \"x\" is not an integer"));

    let o = tsz(&["encode", &csv, "--codec", "lz4"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("unknown codec \"lz4\""));

    let junk = tmp("junk.tsz");
    std::fs::write(&junk, b"PK\x03\x04 not a tsz file").unwrap();
    let o = tsz(&["decode", &junk]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("not a tsz stream"));
}

#[test]
fn compare_verify_and_inspect() {
    let csv = tmp("c.csv");
    // A drifting reading with three decimals: few exact repeats, so the
    // decimal codec beats Chimp128.
    let body: String = (0..5000)
        .map(|i| {
            format!(
                "{},{:.3}\n",
                1_000_000 + 60 * i,
                10.0 + 3.0 * (i as f64 / 7.0).sin() + i as f64 / 1000.0
            )
        })
        .collect();
    std::fs::write(&csv, &body).unwrap();

    let o = tsz(&["compare", &csv]);
    assert!(o.status.success());
    let out = text(&o.stdout);
    assert!(
        out.contains("gorilla") && out.contains("chimp128") && out.contains("decimal 2"),
        "{out}"
    );

    let o = tsz(&["verify", &csv, "--block", "100"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains("gorilla ok, chimp ok, chimp128 ok, auto ok"));

    let f = tmp("c.tsz");
    assert!(tsz(&["encode", &csv, "-o", &f]).status.success());
    let o = tsz(&["inspect", &f]);
    assert!(o.status.success());
    let out = text(&o.stdout);
    assert!(
        out.contains("5,000 points in 2 blocks") && out.contains("value codecs      decimal 2"),
        "{out}"
    );
}

#[test]
fn a_closed_pipe_is_not_a_panic() {
    let csv = tmp("p.csv");
    let body: String = (0..200_000).map(|i| format!("{i},{i}\n")).collect();
    std::fs::write(&csv, &body).unwrap();
    let f = tmp("p.tsz");
    assert!(tsz(&["encode", &csv, "-o", &f]).status.success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_tsz"))
        .args(["decode", &f])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // like `| head` exiting at once
    let o = child.wait_with_output().unwrap();
    assert!(!text(&o.stderr).contains("panicked"), "{}", text(&o.stderr));
}
