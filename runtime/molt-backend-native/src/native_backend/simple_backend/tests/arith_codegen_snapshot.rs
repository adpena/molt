//! Byte-identical codegen proof for the `handle_arith_op` family split.
//!
//! `fc::arith::handle_arith_op` was a ~1.9K-line single-function monolith
//! dispatching every scalar add/sub/mul (+ their in-place and `checked_*`
//! peel forms) inline. It was decomposed into one thin `match` dispatcher
//! delegating to per-family free `fn`s (`handle_add_op`, `handle_checked_add_op`,
//! `handle_checked_mul_op`, `handle_inplace_add_op`, `handle_sub_op`,
//! `handle_inplace_sub_op`, `handle_mul_op`, `handle_inplace_mul_op`), each
//! carrying one arm's body VERBATIM — the same move-only idiom that split
//! `handle_call_op` (`fc::calls`).
//!
//! This test pins the emitted object bytes for a program that routes every
//! extracted family through `SimpleBackend::compile`. The golden digest below
//! was captured at the pre-extraction BASE and must remain byte-identical
//! after the split: a native code generator that changed even one Cranelift
//! instruction would produce different object bytes and fail here. The proof
//! is the emptiness of the byte diff, expressed as a fixed FNV-1a digest +
//! length over the full object image.

use super::*;

/// Deterministic 64-bit FNV-1a over the object image. A single differing
/// codegen byte flips the digest, so equality is a byte-identity assertion.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn binop(kind: &str, lhs: &str, rhs: &str, out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(vec![lhs.to_string(), rhs.to_string()]),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

/// Typed int-primary function: exercises the raw-int carrier lanes of
/// `add`/`sub`/`mul`/`inplace_*` (branchless `iadd`/`isub`/`imul` with deferred
/// overflow) plus the boxed tag-check merge lanes, over `int` params.
fn typed_int_function() -> FunctionIR {
    FunctionIR {
        name: "arith_typed_int".to_string(),
        params: vec!["a".to_string(), "b".to_string()],
        ops: vec![
            binop("add", "a", "b", "s0"),
            binop("sub", "s0", "b", "s1"),
            binop("mul", "s1", "a", "s2"),
            binop("inplace_add", "s2", "b", "s3"),
            binop("inplace_sub", "s3", "a", "s4"),
            binop("inplace_mul", "s4", "b", "s5"),
            OpIR {
                kind: "ret".to_string(),
                var: Some("s5".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
    }
}

/// Generic (untyped) function: forces the boxed tag-check + inline-float +
/// mixed-int/float slow lanes of every family, since operands carry no static
/// scalar representation.
fn generic_function() -> FunctionIR {
    FunctionIR {
        name: "arith_generic".to_string(),
        params: vec!["x".to_string(), "y".to_string()],
        ops: vec![
            binop("add", "x", "y", "g0"),
            binop("sub", "g0", "y", "g1"),
            binop("mul", "g1", "x", "g2"),
            binop("inplace_add", "g2", "y", "g3"),
            binop("inplace_sub", "g3", "x", "g4"),
            binop("inplace_mul", "g4", "y", "g5"),
            OpIR {
                kind: "ret".to_string(),
                var: Some("g5".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    }
}

/// Float-typed function: exercises the float-lane `fadd`/`fsub`/`fmul` fast
/// paths of `add`/`sub`/`mul` and their in-place forms.
fn typed_float_function() -> FunctionIR {
    FunctionIR {
        name: "arith_typed_float".to_string(),
        params: vec!["p".to_string(), "q".to_string()],
        ops: vec![
            binop("add", "p", "q", "f0"),
            binop("sub", "f0", "q", "f1"),
            binop("mul", "f1", "p", "f2"),
            binop("inplace_add", "f2", "q", "f3"),
            binop("inplace_sub", "f3", "p", "f4"),
            binop("inplace_mul", "f4", "q", "f5"),
            OpIR {
                kind: "ret".to_string(),
                var: Some("f5".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["float".to_string(), "float".to_string()]),
        source_file: None,
        is_extern: false,
    }
}

fn compile_arith_families_object() -> Vec<u8> {
    let ir = SimpleIR {
        functions: vec![
            typed_int_function(),
            generic_function(),
            typed_float_function(),
        ],
        profile: None,
    };
    SimpleBackend::new().compile(ir).bytes
}

/// BYTE-IDENTICAL CODEGEN PROOF for the `handle_arith_op` family split.
///
/// The digest + length below were captured at the pre-split BASE
/// (`origin/main` @ 8fac9b5a7, before extracting per-family helpers). After the
/// move-only decomposition the identical program must emit the identical object
/// image. If this assertion fails, the extraction changed native arithmetic
/// codegen — the split is NOT byte-identical and must be corrected until it is.
#[test]
fn arith_op_family_split_is_byte_identical() {
    let bytes = compile_arith_families_object();
    let digest = fnv1a_64(&bytes);
    let len = bytes.len();
    assert_eq!(
        (len, digest),
        (ARITH_GOLDEN_LEN, ARITH_GOLDEN_DIGEST),
        "handle_arith_op family split changed native codegen: object image \
         (len={len}, fnv1a=0x{digest:016x}) differs from the pre-split golden \
         (len={ARITH_GOLDEN_LEN}, fnv1a=0x{ARITH_GOLDEN_DIGEST:016x}). The \
         extraction must be byte-identical — a differing byte means the moved \
         arm bodies do not emit the same Cranelift IR.",
    );
}

// Golden captured at BASE origin/main @ 8fac9b5a7 (pre-extraction, base
// monolithic `handle_arith_op`), via the `arith_codegen_golden_capture` probe
// below. The decomposed dispatcher + per-family helpers must reproduce this
// exact object image.
const ARITH_GOLDEN_LEN: usize = 3423;
const ARITH_GOLDEN_DIGEST: u64 = 0x6960_ca7f_32c1_daae;

/// One-shot capture probe: run with `--ignored --nocapture` to print the
/// golden `(len, digest)` for `ARITH_GOLDEN_LEN` / `ARITH_GOLDEN_DIGEST`.
#[test]
#[ignore = "capture-only: prints the golden digest, does not assert"]
fn arith_codegen_golden_capture() {
    let bytes = compile_arith_families_object();
    println!(
        "ARITH_GOLDEN_LEN = {};\nARITH_GOLDEN_DIGEST = 0x{:016x};",
        bytes.len(),
        fnv1a_64(&bytes)
    );
}
