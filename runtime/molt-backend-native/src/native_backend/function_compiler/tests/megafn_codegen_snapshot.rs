//! Byte-identical codegen snapshot proof for the `handle_call_op` mega-function
//! extraction (behavior-preserving function split of `fc/calls.rs`).
//!
//! Each program is constructed to route through one of the call-family
//! `handle_call_op` arms and is compiled end-to-end via
//! `SimpleBackend::new().compile(ir)`, which emits real Cranelift object bytes.
//! A behavior-preserving extraction MUST produce byte-identical object output.
//!
//! The test prints one `MEGAFN_SNAPSHOT prog=<name> len=<bytes> hash=<hex>`
//! line per program (captured with `--nocapture`) and, when `MEGAFN_OBJ_DIR`
//! is set, writes the raw object bytes to `<dir>/<name>.o` for an exact
//! external byte-diff. The differential proof compares these outputs from the
//! pre-extraction tree against the post-extraction tree.

use super::*;
use std::hash::{Hash, Hasher};

fn const_int(out: &str, v: i64) -> OpIR {
    OpIR {
        kind: "const".to_string(),
        out: Some(out.to_string()),
        value: Some(v),
        ..OpIR::default()
    }
}

fn const_str(out: &str, s: &str) -> OpIR {
    OpIR {
        kind: "const_str".to_string(),
        out: Some(out.to_string()),
        s_value: Some(s.to_string()),
        ..OpIR::default()
    }
}

fn ret(name: &str) -> OpIR {
    OpIR {
        kind: "ret".to_string(),
        args: Some(vec![name.to_string()]),
        ..OpIR::default()
    }
}

fn func(name: &str, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: vec![],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    }
}

/// Programs exercising the `handle_call_op` arms. Each returns a
/// `(name, SimpleIR)` pair. The function set is compiled together so that
/// intra-module known-function facts (direct/guarded dispatch) are realistic.
#[allow(clippy::vec_init_then_push)]
fn call_family_programs() -> Vec<(&'static str, SimpleIR)> {
    let mut progs: Vec<(&'static str, SimpleIR)> = Vec::new();

    // ----- plain `call` to a statically-known defined function (fast/direct + guarded paths).
    progs.push((
        "call_direct_and_guarded",
        SimpleIR {
            functions: vec![
                func("callee_leaf", vec![const_int("k", 3), ret("k")]),
                func(
                    "caller_direct",
                    vec![
                        const_int("a", 1),
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("callee_leaf".to_string()),
                            args: Some(vec!["a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::default()
                        },
                        ret("r"),
                    ],
                ),
            ],
            profile: None,
        },
    ));

    // ----- `call` to an imported (not module-defined) symbol -> outlined guarded call.
    progs.push((
        "call_imported_outlined",
        SimpleIR {
            functions: vec![func(
                "caller_import",
                vec![
                    const_int("a", 5),
                    const_int("b", 6),
                    OpIR {
                        kind: "call".to_string(),
                        s_value: Some("some_external_symbol".to_string()),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_internal` (direct internal dispatch).
    progs.push((
        "call_internal",
        SimpleIR {
            functions: vec![
                func("internal_target", vec![const_int("z", 9), ret("z")]),
                func(
                    "caller_internal",
                    vec![
                        const_int("a", 2),
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("internal_target".to_string()),
                            args: Some(vec!["a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::default()
                        },
                        ret("r"),
                    ],
                ),
            ],
            profile: None,
        },
    ));

    // ----- `call_guarded` (dynamic callee value + guard/merge control flow).
    progs.push((
        "call_guarded",
        SimpleIR {
            functions: vec![
                func("guarded_target", vec![const_int("z", 4), ret("z")]),
                func(
                    "caller_guarded",
                    vec![
                        const_str("callee", "guarded_target"),
                        const_int("a", 7),
                        OpIR {
                            kind: "call_guarded".to_string(),
                            s_value: Some("guarded_target".to_string()),
                            args: Some(vec!["callee".to_string(), "a".to_string()]),
                            out: Some("r".to_string()),
                            ..OpIR::default()
                        },
                        ret("r"),
                    ],
                ),
            ],
            profile: None,
        },
    ));

    // ----- `call_func` (inline probe fast-path: <=3 args, code_id 0).
    progs.push((
        "call_func_inline_probe",
        SimpleIR {
            functions: vec![func(
                "caller_call_func",
                vec![
                    const_str("f", "target_fn"),
                    const_int("a", 1),
                    const_int("b", 2),
                    OpIR {
                        kind: "call_func".to_string(),
                        args: Some(vec!["f".to_string(), "a".to_string(), "b".to_string()]),
                        out: Some("r".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_func` fallback dispatch (>3 args -> molt_call_func_dispatch).
    progs.push((
        "call_func_dispatch",
        SimpleIR {
            functions: vec![func(
                "caller_call_func_many",
                vec![
                    const_str("f", "target_fn"),
                    const_int("a", 1),
                    const_int("b", 2),
                    const_int("c", 3),
                    const_int("d", 4),
                    OpIR {
                        kind: "call_func".to_string(),
                        args: Some(vec![
                            "f".to_string(),
                            "a".to_string(),
                            "b".to_string(),
                            "c".to_string(),
                            "d".to_string(),
                        ]),
                        out: Some("r".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_bind` (consumes a callargs builder; tracked-root scrub path).
    progs.push((
        "call_bind",
        SimpleIR {
            functions: vec![func(
                "caller_call_bind",
                vec![
                    const_str("f", "bound_target"),
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("cargs".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_bind".to_string(),
                        args: Some(vec!["f".to_string(), "cargs".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_indirect`.
    progs.push((
        "call_indirect",
        SimpleIR {
            functions: vec![func(
                "caller_call_indirect",
                vec![
                    const_str("f", "indirect_target"),
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("cargs".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["f".to_string(), "cargs".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_method_ic` (fused instance-method dispatch).
    progs.push((
        "call_method_ic",
        SimpleIR {
            functions: vec![func(
                "caller_method_ic",
                vec![
                    const_str("recv", "obj"),
                    const_int("a", 11),
                    OpIR {
                        kind: "call_method_ic".to_string(),
                        s_value: Some("do_thing".to_string()),
                        args: Some(vec!["recv".to_string(), "a".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_super_method_ic` (fused super().method()).
    progs.push((
        "call_super_method_ic",
        SimpleIR {
            functions: vec![func(
                "caller_super_method_ic",
                vec![
                    const_str("cls", "C"),
                    const_str("selfv", "inst"),
                    const_int("a", 13),
                    OpIR {
                        kind: "call_super_method_ic".to_string(),
                        s_value: Some("do_thing".to_string()),
                        args: Some(vec![
                            "cls".to_string(),
                            "selfv".to_string(),
                            "a".to_string(),
                        ]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_method` generic IC path.
    progs.push((
        "call_method_generic",
        SimpleIR {
            functions: vec![func(
                "caller_call_method",
                vec![
                    const_str("m", "bound"),
                    const_int("a", 15),
                    OpIR {
                        kind: "call_method".to_string(),
                        args: Some(vec!["m".to_string(), "a".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `call_method` fast bound-method dispatch (list.append).
    progs.push((
        "call_method_fast_list_append",
        SimpleIR {
            functions: vec![func(
                "caller_list_append",
                vec![
                    const_str("m", "bound"),
                    const_int("a", 17),
                    OpIR {
                        kind: "call_method".to_string(),
                        s_value: Some("BoundMethod:list:append".to_string()),
                        args: Some(vec!["m".to_string(), "a".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `getargv`.
    progs.push((
        "getargv",
        SimpleIR {
            functions: vec![func(
                "caller_getargv",
                vec![
                    OpIR {
                        kind: "getargv".to_string(),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `getframe`.
    progs.push((
        "getframe",
        SimpleIR {
            functions: vec![func(
                "caller_getframe",
                vec![
                    const_int("depth", 0),
                    OpIR {
                        kind: "getframe".to_string(),
                        args: Some(vec!["depth".to_string()]),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    // ----- `sys_executable`.
    progs.push((
        "sys_executable",
        SimpleIR {
            functions: vec![func(
                "caller_sys_executable",
                vec![
                    OpIR {
                        kind: "sys_executable".to_string(),
                        out: Some("r".to_string()),
                        ..OpIR::default()
                    },
                    ret("r"),
                ],
            )],
            profile: None,
        },
    ));

    progs
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Compile every call-family program, print a stable snapshot line, and (when
/// `MEGAFN_OBJ_DIR` is set) persist the raw object bytes for an exact
/// byte-diff. The pre-extraction and post-extraction trees are compiled with
/// this identical fixture set; a behavior-preserving `handle_call_op` split
/// yields byte-identical object output for each program.
#[test]
fn megafn_call_family_codegen_snapshot() {
    let obj_dir = std::env::var("MEGAFN_OBJ_DIR").ok();
    if let Some(dir) = obj_dir.as_deref() {
        let _ = std::fs::create_dir_all(dir);
    }
    for (name, ir) in call_family_programs() {
        let output = SimpleBackend::new().compile(ir);
        assert!(
            !output.bytes.is_empty(),
            "program {name} produced no object bytes"
        );
        let h = stable_hash(&output.bytes);
        println!(
            "MEGAFN_SNAPSHOT prog={name} len={} hash={:016x}",
            output.bytes.len(),
            h
        );
        if let Some(dir) = obj_dir.as_deref() {
            let path = std::path::Path::new(dir).join(format!("{name}.o"));
            std::fs::write(&path, &output.bytes).expect("write object bytes");
        }
    }
}
