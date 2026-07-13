//! Proves the rust-backend's EMITTED `format_float` prelude formats
//! `repr(float)` / `str(float)` bit-for-bit identically to CPython 3.12, and
//! that a synthetic revert to the old naive `{f}`/`{f:.1}` formatter FAILS this
//! parity. This is the rust-backend end of the single float-format authority
//! `runtime/molt-runtime/src/object/float_repr.rs::repr_float`.
//!
//! The test extracts the `format_float` block from the real emitted prelude
//! (so it can never drift from what the backend actually ships), compiles it
//! with `rustc` into a standalone binary, runs it, and compares against a table
//! of `(f64, repr)` pairs verified against CPython 3.12.
#![cfg(feature = "rust-backend")]

use molt_backend_rust::rust::RustBackend;
use molt_backend_rust::{FunctionIR, OpIR, SimpleIR};
use std::io::Write;
use std::process::Command;

/// Edge-case table verified against CPython 3.12 `repr(float)`. Each entry is
/// `(f64 bit pattern as u64, expected repr)`. Covers: scientific/fixed
/// threshold neighborhoods (1e16 vs 1e17, 1e-4 vs 1e-5), round-half-to-even
/// ties that Rust `{f}` gets wrong, non-finite, -0.0, subnormals, extremes.
const CASES: &[(u64, &str)] = &[
    (0x3fb999999999999au64, "0.1"),
    (0x3ff0000000000000u64, "1.0"),
    (0x4059000000000000u64, "100.0"),
    (0x4341c37937e08000u64, "1e+16"), // decpt 17 -> scientific
    (0x4340000000000000u64, "9007199254740992.0"),
    (0x430c6bf526340000u64, "1000000000000000.0"), // 1e15, decpt 16 -> fixed
    (0x3f1a36e2eb1c432du64, "0.0001"),             // 1e-4 -> fixed
    (0x3ee4f8b588e368f1u64, "1e-05"),              // 1e-5 -> scientific
    (0x7ff0000000000000u64, "inf"),
    (0xfff0000000000000u64, "-inf"),
    (0x7ff8000000000000u64, "nan"),
    (0x0000000000000000u64, "0.0"),
    (0x8000000000000000u64, "-0.0"),
    (0x0000000000000001u64, "5e-324"), // smallest positive subnormal
    (0x7fefffffffffffffu64, "1.7976931348623157e+308"),
    // round-half-to-even ties Rust `{f}` gets wrong on its own:
    (0x42df575484f5b3e8u64, "137839762462415.62"),
    (0x42ed0f618e252b84u64, "255615187364188.12"),
];

/// Emit the real prelude and slice out the `format_float` block (everything
/// between the port BEGIN/END markers). This guarantees the test exercises the
/// exact source the backend ships, not a hand-copy.
fn emitted_format_float_block() -> String {
    let mut backend = RustBackend::new();
    // Any IR that references a float via str/print pulls in `format_float`.
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "return_none".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };
    let src = backend.compile(&ir);
    let begin = src
        .find("// --- BEGIN CPython-exact repr(float)")
        .expect("emitted prelude must contain the ported float formatter");
    let end_marker = "// --- END CPython-exact repr(float) ---";
    let end = src[begin..]
        .find(end_marker)
        .map(|p| begin + p + end_marker.len())
        .expect("emitted prelude must contain the ported float formatter END marker");
    src[begin..end].to_string()
}

/// Compile `program_src` with rustc into `out_dir`, run it, return stdout lines.
fn compile_and_run(program_src: &str, tag: &str) -> Vec<String> {
    let out_dir = std::env::temp_dir().join(format!("molt_float_repr_test_{tag}"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src_path = out_dir.join("prog.rs");
    let bin_path = out_dir.join(if cfg!(windows) { "prog.exe" } else { "prog" });
    let mut f = std::fs::File::create(&src_path).unwrap();
    f.write_all(program_src.as_bytes()).unwrap();
    drop(f);

    let status = Command::new("rustc")
        .arg("-O")
        .arg("--edition")
        .arg("2021")
        .arg("-A")
        .arg("warnings")
        .arg("-o")
        .arg(&bin_path)
        .arg(&src_path)
        .status()
        .expect("rustc must be available to compile the emitted formatter");
    assert!(
        status.success(),
        "emitted formatter must compile with rustc"
    );

    let output = Command::new(&bin_path)
        .output()
        .expect("compiled formatter binary must run");
    assert!(output.status.success(), "formatter binary must exit 0");
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| s.to_string())
        .collect()
}

/// Build a `main` that prints `format_float(bits)` for each case, one per line.
fn program_with(format_float_block: &str) -> String {
    let mut prog = String::new();
    prog.push_str(format_float_block);
    prog.push_str("\nfn main() {\n");
    prog.push_str("    let bits: &[u64] = &[\n");
    for (bits, _) in CASES {
        prog.push_str(&format!("        0x{bits:016x}u64,\n"));
    }
    prog.push_str("    ];\n");
    prog.push_str("    for &b in bits { println!(\"{}\", format_float(f64::from_bits(b))); }\n");
    prog.push_str("}\n");
    prog
}

#[test]
fn emitted_format_float_matches_cpython_repr() {
    let block = emitted_format_float_block();
    // Sanity: the naive formatter must be gone from what we emit.
    assert!(
        !block.contains("format!(\"{f:.1}\")"),
        "emitted formatter must not contain the naive `{{f:.1}}` body"
    );
    assert!(
        block.contains("fn format_float(f: f64) -> String"),
        "emitted block must define format_float"
    );

    let got = compile_and_run(&program_with(&block), "authority");
    assert_eq!(got.len(), CASES.len(), "one output line per case");
    for (line, (bits, want)) in got.iter().zip(CASES.iter()) {
        assert_eq!(
            line, want,
            "format_float(f64::from_bits(0x{bits:016x})) = {line:?}, want {want:?} (CPython repr)"
        );
    }
}

/// Synthetic revert: prove the OLD naive formatter FAILS this parity, so the
/// test has real teeth. If this ever passes, the parity table is toothless.
#[test]
fn synthetic_naive_formatter_fails_cpython_parity() {
    let naive_block = r#"
fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.is_finite() {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}
"#;
    let got = compile_and_run(&program_with(naive_block), "naive");
    let mut mismatches = 0usize;
    for (line, (_bits, want)) in got.iter().zip(CASES.iter()) {
        if line != want {
            mismatches += 1;
        }
    }
    assert!(
        mismatches > 0,
        "the naive formatter must diverge from CPython repr on at least one \
         edge case, otherwise the parity test has no teeth"
    );
}
