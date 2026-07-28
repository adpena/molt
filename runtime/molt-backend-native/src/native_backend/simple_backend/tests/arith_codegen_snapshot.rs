//! Determinism and size guard for the native arithmetic codegen family.
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
//! The original test pinned bytes from a historical move-only decomposition.
//! That baseline predated explicit CFG live-value transport and therefore
//! encoded zero-initialized live ranges as if they were valid codegen. The
//! current guard checks the durable properties instead: deterministic object
//! bytes and a bounded size for the full arithmetic-family stress program.

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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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

#[test]
fn arith_codegen_is_deterministic_and_size_bounded() {
    let first = compile_arith_families_object();
    let second = compile_arith_families_object();
    assert_eq!(
        first, second,
        "native arithmetic codegen must be deterministic"
    );
    assert!(
        first.len() <= ARITH_OBJECT_SIZE_CEILING,
        "native arithmetic stress object grew past its explicit size ceiling: len={}, digest=0x{:016x}",
        first.len(),
        fnv1a_64(&first),
    );
}

// Explicit live transport grows the former 3,423-byte wrong-code baseline to
// 3,713 bytes (+8.47%). Keep 10.3% headroom for deterministic toolchain layout
// movement while making another unbounded live-range/code-size regression red.
const ARITH_OBJECT_SIZE_CEILING: usize = 4096;

/// One-shot capture probe: run with `--ignored --nocapture` to print the
/// current `(len, digest)` when investigating a deliberate codegen change.
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
