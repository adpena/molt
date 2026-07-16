#![cfg(feature = "rust-backend")]
//! Behavior-preservation proof for the `emit_op` per-arm extraction.
//!
//! Compiles a corpus of op-family programs through `RustBackend::compile` and
//! writes the concatenated generated Rust source to
//! `MEGAFN_SNAPSHOT_OUT` (env). The byte-for-byte diff of that file before vs
//! after the `emit_op` split is the proof that the dispatcher rewrite changed
//! no emitted source. Mirrors the native backend's `megafn_codegen_snapshot`
//! object-byte diff.

use molt_backend_rust::rust::RustBackend;
use molt_backend_rust::{FunctionIR, OpIR, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn func(name: &str, params: Vec<&str>, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.into_iter().map(|s| s.to_string()).collect(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
    }
}

/// Op constructors threading the fields each family reads.
fn with_out(mut o: OpIR, out: &str) -> OpIR {
    o.out = Some(out.to_string());
    o
}
fn with_args(mut o: OpIR, args: &[&str]) -> OpIR {
    o.args = Some(args.iter().map(|s| s.to_string()).collect());
    o
}
fn with_value(mut o: OpIR, v: i64) -> OpIR {
    o.value = Some(v);
    o
}
fn with_fvalue(mut o: OpIR, v: f64) -> OpIR {
    o.f_value = Some(v);
    o
}
fn with_svalue(mut o: OpIR, v: &str) -> OpIR {
    o.s_value = Some(v.to_string());
    o
}
fn with_var(mut o: OpIR, v: &str) -> OpIR {
    o.var = Some(v.to_string());
    o
}

/// Each corpus program exercises one op family. Programs are wrapped in a
/// callable function (`molt_prog`) plus a `molt_main` so `compile` runs the
/// full body emission path.
#[allow(clippy::vec_init_then_push)]
fn corpus() -> Vec<(&'static str, SimpleIR)> {
    let main = || func("molt_main", vec![], vec![op("return_none")]);
    let mut out = Vec::new();

    // 1. Constants
    out.push((
        "constants",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec![],
                    vec![
                        with_value(with_out(op("const"), "c_int"), 7),
                        with_fvalue(with_out(op("const_float"), "c_flt"), 1.5),
                        with_svalue(with_out(op("const_str"), "c_str"), "hi"),
                        with_value(with_out(op("const_bool"), "c_b"), 1),
                        with_out(op("const_none"), "c_n"),
                        with_svalue(with_out(op("const_bigint"), "c_bi"), "42"),
                        with_out(op("const_bytes"), "c_by"),
                        with_out(op("const_ellipsis"), "c_el"),
                        with_args(with_out(op("box"), "c_bx"), &["c_int"]),
                        with_args(with_out(op("return"), "r"), &["c_int"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 2. Variable access
    out.push((
        "variables",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["p"],
                    vec![
                        with_var(with_out(op("load_local"), "v0"), "p"),
                        with_var(with_out(op("load_var"), "v1"), "p"),
                        with_args(with_var(op("store_var"), "p"), &["v0"]),
                        with_args(with_value(with_out(op("load"), "v2"), 3), &["v0"]),
                        with_args(with_out(op("closure_load"), "v3"), &["p"]),
                        with_args(with_var(op("store_local"), "p"), &["v1"]),
                        with_args(with_value(op("store"), 2), &["p", "v1"]),
                        with_args(op("closure_store"), &["p", "v2"]),
                        op("phi"),
                        with_args(with_out(op("return"), "r"), &["v2"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 3. Arithmetic + bitwise + unary
    out.push((
        "arithmetic",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["a", "b"],
                    vec![
                        with_args(with_out(op("add"), "s0"), &["a", "b"]),
                        with_args(with_out(op("sub"), "s1"), &["a", "b"]),
                        with_args(with_out(op("mul"), "s2"), &["a", "b"]),
                        with_args(with_out(op("div"), "s3"), &["a", "b"]),
                        with_args(with_out(op("floor_div"), "s4"), &["a", "b"]),
                        with_args(with_out(op("mod"), "s5"), &["a", "b"]),
                        with_args(with_out(op("pow"), "s6"), &["a", "b"]),
                        with_args(with_out(op("neg"), "s7"), &["a"]),
                        with_args(with_out(op("unary_not"), "s8"), &["a"]),
                        with_args(with_out(op("band"), "s9"), &["a", "b"]),
                        with_args(with_out(op("bor"), "s10"), &["a", "b"]),
                        with_args(with_out(op("bxor"), "s11"), &["a", "b"]),
                        with_args(with_out(op("lshift"), "s12"), &["a", "b"]),
                        with_args(with_out(op("rshift"), "s13"), &["a", "b"]),
                        with_args(with_out(op("return"), "r"), &["s0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 4. Comparisons + membership + boolean logic
    out.push((
        "comparisons",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["a", "b"],
                    vec![
                        with_args(with_out(op("eq"), "c0"), &["a", "b"]),
                        with_args(with_out(op("ne"), "c1"), &["a", "b"]),
                        with_args(with_out(op("lt"), "c2"), &["a", "b"]),
                        with_args(with_out(op("le"), "c3"), &["a", "b"]),
                        with_args(with_out(op("gt"), "c4"), &["a", "b"]),
                        with_args(with_out(op("ge"), "c5"), &["a", "b"]),
                        with_args(with_out(op("is"), "c6"), &["a", "b"]),
                        with_args(with_out(op("is_not"), "c7"), &["a", "b"]),
                        with_args(with_out(op("in"), "c8"), &["a", "b"]),
                        with_args(with_out(op("not_in"), "c9"), &["a", "b"]),
                        with_args(with_out(op("contains"), "c10"), &["a", "b"]),
                        with_args(with_out(op("and"), "c11"), &["a", "b"]),
                        with_args(with_out(op("or"), "c12"), &["a", "b"]),
                        with_args(with_out(op("return"), "r"), &["c0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 5. Control flow: if/else/loops
    out.push((
        "control_flow",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["cond"],
                    vec![
                        with_args(op("if"), &["cond"]),
                        op("else"),
                        op("end_if"),
                        with_args(op("if_not"), &["cond"]),
                        op("end_if"),
                        op("loop_start"),
                        with_args(op("loop_break_if_false"), &["cond"]),
                        with_args(op("loop_break_if_true"), &["cond"]),
                        op("loop_break_if_exception"),
                        op("loop_break"),
                        op("loop_continue"),
                        op("loop_end"),
                        op("return_none"),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 6. Iteration + ranges
    out.push((
        "iteration",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["seq"],
                    vec![
                        with_args(with_out(op("iter"), "it"), &["seq"]),
                        with_args(with_out(op("iter_next"), "nx"), &["it"]),
                        with_args(with_out(op("range_new"), "rg"), &["seq", "seq", "seq"]),
                        with_args(op("for_range"), &["i", "seq", "seq", "seq"]),
                        op("end_for"),
                        with_args(with_out(op("for_iter"), "e"), &["seq"]),
                        op("end_for"),
                        op("return_none"),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 7. Function calls
    out.push((
        "calls",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["f", "x"],
                    vec![
                        with_args(with_svalue(with_out(op("call"), "r0"), "helper"), &["x"]),
                        with_args(with_out(op("call"), "r1"), &["f", "x"]),
                        with_args(
                            with_svalue(with_out(op("call_method"), "r2"), "append"),
                            &["x", "x"],
                        ),
                        with_args(
                            with_svalue(with_out(op("call_method"), "r3"), "keys"),
                            &["x"],
                        ),
                        with_args(with_out(op("call_bind"), "r4"), &["f", "x"]),
                        with_args(with_out(op("callargs_new"), "r5"), &["x"]),
                        with_args(op("callargs_push_pos"), &["r5", "x"]),
                        with_args(op("callargs_expand_star"), &["r5", "x"]),
                        with_svalue(with_out(op("func_new"), "r6"), "helper"),
                        with_svalue(with_out(op("builtin_func"), "r7"), "len"),
                        with_args(with_out(op("return"), "r"), &["r0"]),
                    ],
                ),
                func(
                    "helper",
                    vec!["a"],
                    vec![with_args(with_out(op("return"), "r"), &["a"])],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 8. Builtins (numeric/str casts)
    out.push((
        "builtins",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["x"],
                    vec![
                        with_args(op("print"), &["x"]),
                        with_args(with_out(op("len"), "b0"), &["x"]),
                        with_args(with_out(op("int"), "b1"), &["x"]),
                        with_args(with_out(op("int_from_obj"), "b2"), &["x"]),
                        with_args(with_out(op("int_from_str_of_obj"), "b3"), &["x", "x", "x"]),
                        with_args(with_out(op("float"), "b4"), &["x"]),
                        with_args(with_out(op("str"), "b5"), &["x"]),
                        with_args(with_out(op("bool"), "b6"), &["x"]),
                        with_args(with_out(op("chr"), "b7"), &["x"]),
                        with_args(with_out(op("ord"), "b8"), &["x"]),
                        with_args(with_out(op("ord_at"), "b9"), &["x", "x"]),
                        with_args(with_out(op("abs"), "b10"), &["x"]),
                        with_args(with_out(op("return"), "r"), &["b0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 9. Collections + subscript + attributes
    out.push((
        "collections",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["k", "v"],
                    vec![
                        with_args(with_out(op("build_list"), "l0"), &["k", "v"]),
                        with_args(with_out(op("build_dict"), "d0"), &["k", "v"]),
                        with_args(op("list_append"), &["l0", "v"]),
                        with_args(with_out(op("get_item"), "g0"), &["l0", "k"]),
                        with_args(with_out(op("dict_get"), "g1"), &["d0", "k", "v"]),
                        with_args(op("set_item"), &["l0", "k", "v"]),
                        with_args(op("dict_set"), &["d0", "k", "v"]),
                        with_args(
                            with_svalue(with_out(op("get_attr"), "a0"), "field"),
                            &["l0"],
                        ),
                        with_args(with_out(op("get_attr_name"), "a1"), &["l0", "k"]),
                        with_args(
                            with_out(op("get_attr_name_default"), "a2"),
                            &["l0", "k", "v"],
                        ),
                        with_args(with_svalue(op("set_attr"), "field"), &["l0", "v"]),
                        with_args(with_out(op("return"), "r"), &["l0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 10. Enumerate/zip/sorted + range builtin
    out.push((
        "sequence_builtins",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["a", "b"],
                    vec![
                        with_args(with_out(op("enumerate"), "e0"), &["a", "b"]),
                        with_args(with_out(op("zip"), "e1"), &["a", "b"]),
                        with_args(with_out(op("sorted"), "e2"), &["a"]),
                        with_args(with_out(op("reversed"), "e3"), &["a"]),
                        with_args(with_out(op("sum"), "e4"), &["a"]),
                        with_args(with_out(op("any"), "e5"), &["a"]),
                        with_args(with_out(op("all"), "e6"), &["a"]),
                        with_args(with_out(op("range"), "e7"), &["a", "b"]),
                        with_args(with_out(op("return"), "r"), &["e0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 11. Modules
    out.push((
        "modules",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["name", "mod_"],
                    vec![
                        with_out(op("module_new"), "m0"),
                        with_args(with_out(op("module_cache_get"), "m1"), &["name"]),
                        with_args(with_out(op("module_cache_set"), "m2"), &["name", "mod_"]),
                        with_args(with_out(op("module_cache_del"), "m3"), &["name"]),
                        with_args(with_out(op("module_import"), "m4"), &["name"]),
                        with_args(
                            with_svalue(with_out(op("module_get_attr"), "m5"), "attr"),
                            &["mod_"],
                        ),
                        with_args(op("module_set_attr"), &["mod_", "name", "name"]),
                        with_args(with_out(op("return"), "r"), &["m0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 12. Exceptions/frame/trace
    out.push((
        "exceptions",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["exc"],
                    vec![
                        with_out(op("exception_last"), "x0"),
                        with_out(op("exception_last_pending"), "x1"),
                        with_out(op("exception_stack_depth"), "x2"),
                        with_out(op("exception_stack_enter"), "x3"),
                        with_out(op("exception_clear"), "x4"),
                        with_args(op("exception_stack_exit"), &["x2"]),
                        with_args(op("exception_stack_set_depth"), &["x2"]),
                        op("exception_stack_clear"),
                        with_args(op("exception_set_last"), &["exc"]),
                        with_out(op("exception_active"), "x5"),
                        with_value(op("trace_enter_slot"), 1),
                        op("trace_exit"),
                        with_args(op("frame_locals_set"), &["exc"]),
                        op("try_start"),
                        op("try_end"),
                        op("return_none"),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 13. Strings + sequences/tuples
    out.push((
        "strings_sequences",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["a", "b"],
                    vec![
                        with_args(with_out(op("format_string"), "s0"), &["a", "b"]),
                        with_args(with_out(op("str_from_obj"), "s1"), &["a"]),
                        with_args(with_out(op("repr_from_obj"), "s2"), &["a"]),
                        with_args(with_out(op("tuple_new"), "s3"), &["a", "b"]),
                        with_args(with_out(op("list_fill_new"), "s4"), &["a", "b"]),
                        with_value(with_args(op("unpack_sequence"), &["a", "u0", "u1"]), 2),
                        with_args(with_out(op("string_join"), "s5"), &["a", "b"]),
                        with_args(with_out(op("return"), "r"), &["s0"]),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 14. Markers/no-ops + refcount + unsupported diagnostics
    out.push((
        "markers_refcount",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog",
                    vec!["x"],
                    vec![
                        op("nop"),
                        with_args(op("br_if"), &["x"]),
                        with_args(with_out(op("inc_ref"), "y0"), &["x"]),
                        with_args(op("dec_ref"), &["x"]),
                        with_args(with_out(op("alloc_instance"), "y1"), &["x"]),
                        with_out(op("class_new"), "y2"),
                        with_out(op("this_op_does_not_exist_anywhere"), "y3"),
                        op("return_none"),
                    ],
                ),
                main(),
            ],
            profile: None,
        },
    ));

    // 15. Return variants
    out.push((
        "returns",
        SimpleIR {
            functions: vec![
                func(
                    "molt_prog_a",
                    vec!["x"],
                    vec![with_args(with_out(op("return"), "r"), &["x"])],
                ),
                func("molt_prog_b", vec!["x"], vec![with_var(op("return"), "x")]),
                func("molt_prog_c", vec![], vec![op("ret_none")]),
                main(),
            ],
            profile: None,
        },
    ));

    out
}

#[test]
fn emit_op_corpus_source_snapshot() {
    let mut buf = String::new();
    for (name, ir) in corpus() {
        let mut backend = RustBackend::new();
        let source = backend
            .compile_checked(&ir)
            .unwrap_or_else(|err| panic!("{name} must be fully supported: {err}"));
        buf.push_str(&format!("===== PROGRAM: {name} =====\n"));
        buf.push_str(&source);
        buf.push('\n');
    }

    if let Ok(path) = std::env::var("MEGAFN_SNAPSHOT_OUT") {
        std::fs::write(&path, &buf).expect("write snapshot");
        eprintln!("wrote snapshot ({} bytes) to {path}", buf.len());
    } else {
        // Still assert non-empty so the test is meaningful without the env var.
        assert!(!buf.is_empty());
    }
}
