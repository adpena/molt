//! Executable-dispatch proof for `module_attr` native callable exports on the
//! native (Cranelift) backend.
//!
//! The five scipy.ndimage witness ops (`distance_transform_edt`,
//! `gaussian_filter`, `label`, `maximum_filter`, `minimum_filter`) lower to
//! `invoke_ffi` with a `module_attr` binding and either `molt.object_call_v1`
//! (positional payload) or `molt.object_callargs_v1` (a pre-built callargs
//! object). The native backend must route these through the runtime
//! `molt_invoke_ffi_ic` object-call inline cache — exactly as the WASM backend
//! does in `wasm/op_loop/call_ops/dynamic.rs` — instead of panicking. These
//! tests compile end-to-end via `SimpleBackend::new().compile(ir)` (real
//! Cranelift object bytes) and prove:
//!   1. module_attr object_call / object_callargs exports compile and emit a
//!      `molt_invoke_ffi_ic` relocation (the executable dispatch symbol);
//!   2. direct-symbol object, memory, and PyInit ABIs emit relocations to the
//!      manifest-owned symbols with the same ABI contracts as WASM;
//!   3. malformed arity and missing-symbol inputs still fail closed.

use crate::native_backend::simple_backend::tests::{
    compile_selected_functions_direct, emit_direct_object,
};
use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

/// The runtime object-call symbol every executable native callable dispatch
/// must reference. Its ASCII name appears in the emitted object symbol table
/// whenever the backend wires the `molt_invoke_ffi_ic` import.
const INVOKE_FFI_IC_SYMBOL: &[u8] = b"molt_invoke_ffi_ic";

fn const_int(out: &str, v: i64) -> OpIR {
    OpIR {
        kind: "const".to_string(),
        out: Some(out.to_string()),
        value: Some(v),
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

/// Build a single-function program that invokes `export_name` through
/// `invoke_ffi` with the given `binding`/`abi` and positional/callargs args.
fn native_callable_program(
    export_name: &str,
    binding: &str,
    abi: &str,
    symbol: Option<&str>,
    arg_names: &[&str],
) -> SimpleIR {
    let mut ops = Vec::new();
    for (idx, name) in arg_names.iter().enumerate() {
        ops.push(const_int(name, idx as i64 + 1));
    }
    ops.push(OpIR {
        kind: "invoke_ffi".to_string(),
        out: Some("result".to_string()),
        args: Some(arg_names.iter().map(|s| s.to_string()).collect()),
        native_callable_export: Some(export_name.to_string()),
        native_callable_binding: Some(binding.to_string()),
        native_callable_abi: Some(abi.to_string()),
        native_callable_symbol: symbol.map(|s| s.to_string()),
        ..OpIR::default()
    });
    ops.push(ret("result"));
    SimpleIR {
        functions: vec![FunctionIR {
            name: "native_callable_dispatch".to_string(),
            params: vec![],
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        }],
        profile: None,
    }
}

fn object_contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

fn real_rustc() -> Option<PathBuf> {
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"));
    let available = Command::new(&rustc)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if available {
        return Some(rustc);
    }
    if std::env::var_os("CI").is_some()
        || std::env::var_os("MOLT_REQUIRE_REAL_NATIVE_LINK_TESTS").is_some()
        || std::env::var_os("MOLT_REQUIRE_REAL_NATIVE_CALLABLE_EXECUTION_TESTS").is_some()
    {
        panic!("real native final-link proof is required but rustc is unavailable");
    }
    eprintln!(
        "SKIP real native final-link proof: rustc is unavailable; set \
         MOLT_REQUIRE_REAL_NATIVE_LINK_TESTS=1 to make this a hard failure"
    );
    None
}

fn run_checked(command: &mut Command, purpose: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{purpose}: failed to start: {error}"));
    assert!(
        output.status.success(),
        "{purpose}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn native_provider_archive_path(temp: &Path) -> PathBuf {
    if cfg!(windows) {
        temp.join("native_callable_provider.lib")
    } else {
        temp.join("libnative_callable_provider.a")
    }
}

fn link_and_run_native_object(
    rustc: &Path,
    artifact_name: &str,
    object_bytes: Vec<u8>,
    provider_source_text: &str,
    harness_source_text: &str,
    purpose: &str,
) {
    let artifacts =
        cargo_test_artifacts::CargoTestArtifacts::new(artifact_name).unwrap_or_else(|error| {
            panic!("create {purpose} outputs within Cargo image custody: {error:?}")
        });
    let temp = artifacts.path();
    let app_object = temp.join("native_callable_app.o");
    let provider_source = temp.join("provider.rs");
    let provider_archive = native_provider_archive_path(temp);
    let harness_source = temp.join("harness.rs");
    let executable = temp.join(if cfg!(windows) {
        "native_callable_execution.exe"
    } else {
        "native_callable_execution"
    });
    fs::write(&app_object, object_bytes).expect("write Cranelift app object");
    fs::write(&provider_source, provider_source_text).expect("write native provider source");
    run_checked(
        artifacts
            .command(rustc)
            .expect("resolve fixture compiler")
            .arg("--edition=2021")
            .arg("--crate-name=native_callable_provider")
            .arg("--crate-type=rlib")
            .arg("-Cpanic=abort")
            .arg(artifacts.argument("", &provider_source).unwrap())
            .arg("-o")
            .arg(artifacts.argument("", &provider_archive).unwrap()),
        &format!("compile {purpose} provider static archive"),
    );
    fs::write(&harness_source, harness_source_text).expect("write native execution harness");
    run_checked(
        artifacts
            .command(rustc)
            .expect("resolve fixture compiler")
            .arg("--edition=2021")
            .arg(artifacts.argument("", &harness_source).unwrap())
            .arg("-C")
            .arg(artifacts.argument("link-arg=", &app_object).unwrap())
            .arg("-C")
            .arg(artifacts.argument("link-arg=", &provider_archive).unwrap())
            .arg("-o")
            .arg(artifacts.argument("", &executable).unwrap()),
        &format!("final-link Cranelift object with {purpose} provider archive"),
    );
    run_checked(
        &mut Command::new(&executable),
        &format!("execute final-linked {purpose} binary"),
    );
}

#[test]
fn native_module_attr_object_call_dispatches_through_runtime_ffi() {
    // `distance_transform_edt(mask)` form: object_call_v1, callable + one
    // positional arg. The dispatch materializes a callargs builder and invokes.
    let ir = native_callable_program(
        "scipy.ndimage.distance_transform_edt",
        "module_attr",
        "molt.object_call_v1",
        None,
        &["callable_obj", "mask"],
    );

    let output = SimpleBackend::new().compile(ir);

    assert!(
        !output.bytes.is_empty(),
        "module_attr object_call native callable dispatch must emit object bytes, not panic"
    );
    assert!(
        object_contains(&output.bytes, INVOKE_FFI_IC_SYMBOL),
        "module_attr object_call dispatch must reference the runtime {} object-call symbol",
        String::from_utf8_lossy(INVOKE_FFI_IC_SYMBOL)
    );
}

#[test]
fn native_module_attr_object_callargs_dispatches_through_runtime_ffi() {
    // `gaussian_filter(mask, sigma=1.5)` form: object_callargs_v1, callable +
    // one pre-built callargs payload object. The dispatch forwards args[1]
    // directly as the callargs pointer (fixed arity 1).
    let ir = native_callable_program(
        "scipy.ndimage.gaussian_filter",
        "module_attr",
        "molt.object_callargs_v1",
        None,
        &["callable_obj", "callargs_payload"],
    );

    let output = SimpleBackend::new().compile(ir);

    assert!(
        !output.bytes.is_empty(),
        "module_attr object_callargs native callable dispatch must emit object bytes, not panic"
    );
    assert!(
        object_contains(&output.bytes, INVOKE_FFI_IC_SYMBOL),
        "module_attr object_callargs dispatch must reference the runtime {} object-call symbol",
        String::from_utf8_lossy(INVOKE_FFI_IC_SYMBOL)
    );
}

#[test]
#[should_panic(expected = "molt.object_callargs_v1")]
fn native_module_attr_object_callargs_rejects_extra_payload() {
    // Fixed-arity guard: object_callargs must carry exactly one callargs
    // payload; an extra positional arg is a lowering contract violation. It
    // fails closed rather than silently building a wrong call — the shared TIR
    // verifier catches the arity drift before backend codegen (defense in
    // depth), and the backend's own `expects the callable handle plus exactly
    // one callargs payload` guard is the second line if the op ever bypasses
    // verification.
    let ir = native_callable_program(
        "scipy.ndimage.gaussian_filter",
        "module_attr",
        "molt.object_callargs_v1",
        None,
        &["callable_obj", "callargs_payload", "stray_extra"],
    );
    let _ = SimpleBackend::new().compile(ir);
}

#[test]
fn native_direct_symbol_object_call_emits_relocation() {
    let symbol = "molt_native_object_call_probe";
    let ir = native_callable_program(
        "scipy.ndimage.distance_transform_edt",
        "direct_symbol",
        "molt.object_call_v1",
        Some(symbol),
        &["payload"],
    );

    let output = SimpleBackend::new().compile(ir);
    assert!(object_contains(&output.bytes, symbol.as_bytes()));
}

#[test]
fn native_direct_symbol_object_call_links_provider_archive_and_executes() {
    const SYMBOL: &str = "molt_native_object_call_execution_probe";
    const SENTINEL: u64 = 0x45A1_7E57_D15C_A11E;
    let Some(rustc) = real_rustc() else {
        return;
    };
    let ir = native_callable_program(
        "native_probe.execute",
        "direct_symbol",
        "molt.object_call_v1",
        Some(SYMBOL),
        &["payload"],
    );
    let output = SimpleBackend::new().compile(ir);
    let provider_source = format!(
        r#"#![no_std]
#[export_name = "{generated_object_abi_symbol}"]
pub static GENERATED_OBJECT_ABI: u8 = 0;
static EXCEPTION_PENDING: u8 = 0;
#[no_mangle]
pub extern "C" fn molt_dec_ref(_: u64) {{}}
#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(_: u64) {{}}
#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(_: u64) {{}}
#[no_mangle]
pub extern "C" fn molt_exception_pending_fast() -> u64 {{ 0 }}
#[no_mangle]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {{
    core::ptr::addr_of!(EXCEPTION_PENDING) as u64
}}
#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {{ value as u64 }}
#[no_mangle]
pub extern "C" fn molt_async_work_poll_and_exception_pending() -> u64 {{ 0 }}
#[no_mangle]
pub extern "C" fn {SYMBOL}(_: u64) -> u64 {{ {SENTINEL}u64 }}
"#,
        generated_object_abi_symbol = molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
    );
    let harness_source = format!(
        "extern \"C\" {{ fn native_callable_dispatch() -> u64; }}\n\
             fn main() {{ let actual = unsafe {{ native_callable_dispatch() }}; \
             assert_eq!(actual, {SENTINEL}u64, \"direct-symbol ABI returned {{actual:#x}}\"); }}\n"
    );
    link_and_run_native_object(
        &rustc,
        "native-callable-link",
        output.bytes,
        &provider_source,
        &harness_source,
        "native callable",
    );
}

fn cleanup_oracle_function(
    name: &str,
    params: &[&str],
    param_types: Option<&[&str]>,
    ops: Vec<OpIR>,
) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.iter().map(|name| (*name).to_string()).collect(),
        ops,
        param_types: param_types.map(|types| {
            types
                .iter()
                .map(|type_name| (*type_name).to_string())
                .collect()
        }),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }
}

fn cleanup_classmethod_new(input: &str, out: &str) -> OpIR {
    OpIR {
        kind: "classmethod_new".into(),
        args: Some(vec![input.into()]),
        out: Some(out.into()),
        ..OpIR::default()
    }
}

fn cleanup_ret_void() -> OpIR {
    OpIR {
        kind: "ret_void".into(),
        ..OpIR::default()
    }
}

fn cleanup_jump(label: i64) -> OpIR {
    OpIR {
        kind: "jump".into(),
        value: Some(label),
        ..OpIR::default()
    }
}

fn cleanup_label(label: i64) -> OpIR {
    OpIR {
        kind: "label".into(),
        value: Some(label),
        ..OpIR::default()
    }
}

fn cleanup_br_if(condition: &str, label: i64) -> OpIR {
    OpIR {
        kind: "br_if".into(),
        args: Some(vec![condition.into()]),
        value: Some(label),
        ..OpIR::default()
    }
}

fn cleanup_owned_alias_function(name: &str, alias_kind: &str) -> FunctionIR {
    cleanup_oracle_function(
        name,
        &[],
        None,
        vec![
            OpIR {
                kind: "const_none".into(),
                out: Some("borrowed".into()),
                ..OpIR::default()
            },
            cleanup_classmethod_new("borrowed", "owner"),
            OpIR {
                kind: alias_kind.into(),
                args: Some(vec!["owner".into()]),
                out: Some("alias".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "is".into(),
                args: Some(vec!["alias".into(), "borrowed".into()]),
                out: Some("alias_is_none".into()),
                ..OpIR::default()
            },
            cleanup_ret_void(),
        ],
    )
}

fn cleanup_result_store_function(name: &str, first_owner: &str, second_owner: &str) -> FunctionIR {
    cleanup_oracle_function(
        name,
        &[],
        None,
        vec![
            OpIR {
                kind: "const_none".into(),
                out: Some("borrowed".into()),
                ..OpIR::default()
            },
            cleanup_classmethod_new("borrowed", first_owner),
            OpIR {
                kind: "store_var".into(),
                var: Some("local".into()),
                out: Some("first_result".into()),
                args: Some(vec![first_owner.into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".into(),
                var: Some("local".into()),
                out: Some("snapshot".into()),
                ..OpIR::default()
            },
            cleanup_classmethod_new("borrowed", second_owner),
            OpIR {
                kind: "store_var".into(),
                var: Some("local".into()),
                out: Some("second_result".into()),
                args: Some(vec![second_owner.into()]),
                ..OpIR::default()
            },
            ret("snapshot"),
        ],
    )
}

#[test]
fn native_value_tracking_cleanup_matrix_links_and_executes_once() {
    const TARGET_NAMES: &[&str] = &[
        "cleanup_dynamic_successors",
        "cleanup_explicit_or_sibling",
        "cleanup_common_predecessor",
        "cleanup_return_transfer",
        "cleanup_transparent_copy",
        "cleanup_binding_alias",
        "cleanup_box_alias",
        "cleanup_unbox_alias",
        "cleanup_loop_rearm",
        "cleanup_return_borrowed",
        "cleanup_rebind_parameter",
        "cleanup_slot_snapshot_rebind",
        "cleanup_optional_rebind_return",
        "cleanup_explicit_credit",
        "cleanup_conditional_credit",
        "cleanup_iterator_multi_results",
        "cleanup_local_result_store_snapshot",
        "cleanup_explicit_credit_rebind",
        "cleanup_parameter_snapshot_rebind",
        "cleanup_mutable_source_result_store_snapshot",
    ];
    let Some(rustc) = real_rustc() else {
        return;
    };

    let functions = vec![
        cleanup_oracle_function(
            TARGET_NAMES[0],
            &["condition"],
            Some(&["dyn"]),
            vec![
                cleanup_jump(10),
                cleanup_label(10),
                cleanup_classmethod_new("condition", "owner"),
                cleanup_br_if("condition", 20),
                cleanup_ret_void(),
                cleanup_label(20),
                OpIR {
                    kind: "const_none".into(),
                    out: Some("none_value".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".into(),
                    args: Some(vec!["owner".into(), "none_value".into()]),
                    out: Some("owner_is_none".into()),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[1],
            &["condition"],
            Some(&["dyn"]),
            vec![
                cleanup_classmethod_new("condition", "owner"),
                cleanup_br_if("condition", 30),
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_jump(40),
                cleanup_label(30),
                cleanup_jump(40),
                cleanup_label(40),
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[2],
            &["condition"],
            Some(&["dyn"]),
            vec![
                cleanup_classmethod_new("condition", "owner"),
                cleanup_br_if("condition", 50),
                cleanup_ret_void(),
                cleanup_label(50),
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[3],
            &["condition"],
            Some(&["dyn"]),
            vec![
                cleanup_classmethod_new("condition", "owner"),
                cleanup_br_if("condition", 60),
                cleanup_ret_void(),
                cleanup_label(60),
                ret("owner"),
            ],
        ),
        cleanup_owned_alias_function(TARGET_NAMES[4], "copy"),
        cleanup_owned_alias_function(TARGET_NAMES[5], "binding_alias"),
        cleanup_owned_alias_function(TARGET_NAMES[6], "box"),
        cleanup_owned_alias_function(TARGET_NAMES[7], "unbox"),
        cleanup_oracle_function(
            TARGET_NAMES[8],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_none".into(),
                    out: Some("borrowed".into()),
                    ..OpIR::default()
                },
                const_int("index", 0),
                const_int("limit", 2),
                const_int("one", 1),
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["index".into(), "limit".into()]),
                    out: Some("keep_going".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["keep_going".into()]),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "loop_owner"),
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["index".into(), "one".into()]),
                    out: Some("index".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[9],
            &["borrowed"],
            Some(&["dyn"]),
            vec![ret("borrowed")],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[10],
            &["owner", "condition"],
            Some(&["dyn", "dyn"]),
            vec![
                cleanup_classmethod_new("owner", "owner"),
                cleanup_br_if("condition", 70),
                cleanup_ret_void(),
                cleanup_label(70),
                ret("owner"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[11],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_none".into(),
                    out: Some("borrowed".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "first_owner"),
                OpIR {
                    kind: "store_var".into(),
                    var: Some("_bb1_arg0".into()),
                    args: Some(vec!["first_owner".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".into(),
                    var: Some("_bb1_arg0".into()),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "second_owner"),
                OpIR {
                    kind: "store_var".into(),
                    var: Some("_bb1_arg0".into()),
                    args: Some(vec!["second_owner".into()]),
                    ..OpIR::default()
                },
                ret("snapshot"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[12],
            &["owner", "condition"],
            Some(&["dyn", "dyn"]),
            vec![
                cleanup_br_if("condition", 80),
                cleanup_jump(90),
                cleanup_label(80),
                cleanup_classmethod_new("owner", "owner"),
                cleanup_jump(90),
                cleanup_label(90),
                ret("owner"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[13],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_none".into(),
                    out: Some("borrowed".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "owner"),
                OpIR {
                    kind: "inc_ref".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                // Keep the retained owner observable between the two explicit
                // operations; adjacent inc/release pairs are canonically elided.
                OpIR {
                    kind: "is".into(),
                    args: Some(vec!["owner".into(), "borrowed".into()]),
                    out: Some("owner_is_none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[14],
            &["condition"],
            Some(&["dyn"]),
            vec![
                cleanup_classmethod_new("condition", "owner"),
                cleanup_br_if("condition", 100),
                cleanup_jump(110),
                cleanup_label(100),
                OpIR {
                    kind: "inc_ref".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_jump(110),
                cleanup_label(110),
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[15],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next_unboxed".into(),
                    args: Some(vec!["iterator".into()]),
                    var: Some("pair".into()),
                    out: Some("done".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".into(),
                    args: Some(vec!["pair".into(), "left".into(), "right".into()]),
                    value: Some(2),
                    ..OpIR::default()
                },
                ret("pair"),
            ],
        ),
        cleanup_result_store_function(TARGET_NAMES[16], "first_owner", "second_owner"),
        cleanup_oracle_function(
            TARGET_NAMES[17],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_none".into(),
                    out: Some("borrowed".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "owner"),
                OpIR {
                    kind: "binding_alias".into(),
                    args: Some(vec!["owner".into()]),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "inc_ref".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("borrowed", "owner"),
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                // The surviving old-value handle balances its own credit and
                // the explicit external credit from before the rebind.
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["snapshot".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "release".into(),
                    args: Some(vec!["snapshot".into()]),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[18],
            &["owner"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "copy".into(),
                    args: Some(vec!["owner".into()]),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("owner", "owner"),
                ret("snapshot"),
            ],
        ),
        cleanup_result_store_function(TARGET_NAMES[19], "owner", "owner"),
    ];
    assert!(
        functions
            .iter()
            .flat_map(|function| &function.ops)
            .all(|op| op.kind != "drop_inserted"),
        "the executable oracle must exercise NativeValueTracking, not TIR drop authority"
    );

    let backend = compile_selected_functions_direct(functions, TARGET_NAMES);
    let object_bytes = emit_direct_object(backend);
    assert!(!object_bytes.is_empty());
    assert!(
        object_contains(&object_bytes, b"molt_dec_ref_obj"),
        "cleanup oracle object must carry the executable release relocation"
    );

    let provider_source = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering};

const MAX_OWNERS: usize = 64;
const PTR_TAG: u64 = @PTR_TAG@;
const INT_TAG: u64 = @INT_TAG@;
const INT_MASK: u64 = @INT_MASK@;
const BOXED_FALSE: u64 = @BOXED_FALSE@;
const BOXED_TRUE: u64 = @BOXED_TRUE@;
const BOXED_NONE: u64 = @BOXED_NONE@;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static INC_REFS: AtomicU64 = AtomicU64::new(0);
static DEC_REFS: AtomicU64 = AtomicU64::new(0);
static LIVE_REFS: AtomicU64 = AtomicU64::new(0);
static EXCEPTION_PENDING: u8 = 0;
static mut TOKENS: [u64; MAX_OWNERS] = [0; MAX_OWNERS];
static mut REFS: [u64; MAX_OWNERS] = [0; MAX_OWNERS];

#[export_name = "@GENERATED_OBJECT_ABI_SYMBOL@"]
pub static GENERATED_OBJECT_ABI: u8 = 0;

fn owner_index(value: u64) -> Option<usize> {
    let allocated = ALLOCATIONS.load(Ordering::SeqCst) as usize;
    let mut index = 0;
    while index < allocated {
        if unsafe { TOKENS[index] } == value {
            return Some(index);
        }
        index += 1;
    }
    None
}

#[no_mangle]
pub extern "C" fn cleanup_oracle_reset() {
    assert_eq!(LIVE_REFS.load(Ordering::SeqCst), 0, "reset with leaked owner references");
    let mut index = 0;
    while index < MAX_OWNERS {
        unsafe {
            TOKENS[index] = 0;
            REFS[index] = 0;
        }
        index += 1;
    }
    ALLOCATIONS.store(0, Ordering::SeqCst);
    INC_REFS.store(0, Ordering::SeqCst);
    DEC_REFS.store(0, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn cleanup_oracle_allocations() -> u64 { ALLOCATIONS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_oracle_inc_refs() -> u64 { INC_REFS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_oracle_dec_refs() -> u64 { DEC_REFS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_oracle_live_refs() -> u64 { LIVE_REFS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_oracle_owner(index: u64) -> u64 {
    assert!(index < ALLOCATIONS.load(Ordering::SeqCst));
    unsafe { TOKENS[index as usize] }
}

#[no_mangle]
pub extern "C" fn molt_classmethod_new(_: u64) -> u64 {
    let index = ALLOCATIONS.fetch_add(1, Ordering::SeqCst) as usize;
    assert!(index < MAX_OWNERS, "cleanup oracle owner table exhausted");
    let token = PTR_TAG | (0x1000 + ((index as u64 + 1) << 4));
    unsafe {
        TOKENS[index] = token;
        REFS[index] = 1;
    }
    LIVE_REFS.fetch_add(1, Ordering::SeqCst);
    token
}

#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(value: u64) {
    let Some(index) = owner_index(value) else { return; };
    unsafe {
        assert_ne!(REFS[index], 0, "retain after final release");
        REFS[index] += 1;
    }
    INC_REFS.fetch_add(1, Ordering::SeqCst);
    LIVE_REFS.fetch_add(1, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(value: u64) {
    let Some(index) = owner_index(value) else { return; };
    unsafe {
        assert_ne!(REFS[index], 0, "duplicate owner release");
        REFS[index] -= 1;
    }
    DEC_REFS.fetch_add(1, Ordering::SeqCst);
    LIVE_REFS.fetch_sub(1, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn molt_dec_ref(value: u64) { molt_dec_ref_obj(value); }
// These providers model only the owned-result ABI, not iterator semantics.
// The input token represents the materialized pair and must retain its identity.
#[no_mangle]
pub unsafe extern "C" fn molt_iter_next_unboxed(pair: u64, output: u64) -> u64 {
    molt_inc_ref_obj(pair);
    *(output as *mut u64) = pair;
    BOXED_FALSE
}
#[no_mangle]
pub unsafe extern "C" fn molt_unpack_sequence(pair: u64, count: u64, output: u64) -> u64 {
    let index = owner_index(pair).expect("unpack must observe the materialized pair");
    assert_ne!(REFS[index], 0, "unpack read a released pair");
    assert_eq!(count, 2);
    let output = output as *mut u64;
    *output = molt_classmethod_new(BOXED_NONE);
    *output.add(1) = molt_classmethod_new(BOXED_NONE);
    BOXED_NONE
}
#[no_mangle]
pub extern "C" fn molt_exception_pending_fast() -> u64 { 0 }
#[no_mangle]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {
    core::ptr::addr_of!(EXCEPTION_PENDING) as u64
}
#[no_mangle]
pub extern "C" fn molt_async_work_poll_and_exception_pending() -> u64 { 0 }
#[no_mangle]
pub extern "C" fn molt_is_truthy(value: u64) -> u64 {
    u64::from(value != BOXED_FALSE)
}
#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    INT_TAG | ((value as u64) & INT_MASK)
}
#[no_mangle]
pub extern "C" fn molt_lt(lhs: u64, rhs: u64) -> u64 {
    if (lhs & INT_MASK) < (rhs & INT_MASK) { BOXED_TRUE } else { BOXED_FALSE }
}
#[no_mangle]
pub extern "C" fn molt_add(lhs: u64, rhs: u64) -> u64 {
    INT_TAG | ((lhs.wrapping_add(rhs)) & INT_MASK)
}
"#
    .replace(
        "@GENERATED_OBJECT_ABI_SYMBOL@",
        molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
    )
    .replace(
        "@PTR_TAG@",
        &(molt_codegen_abi::box_ptr_bits(0) as u64).to_string(),
    )
    .replace(
        "@INT_TAG@",
        &(molt_codegen_abi::box_int_bits(0) as u64).to_string(),
    )
    .replace("@INT_MASK@", &molt_codegen_abi::INT_MASK.to_string())
    .replace(
        "@BOXED_NONE@",
        &(molt_codegen_abi::box_none_bits() as u64).to_string(),
    )
    .replace(
        "@BOXED_FALSE@",
        &(molt_codegen_abi::box_bool_bits(0) as u64).to_string(),
    )
    .replace(
        "@BOXED_TRUE@",
        &(molt_codegen_abi::box_bool_bits(1) as u64).to_string(),
    );
    let harness_source = r#"extern "C" {
    fn cleanup_dynamic_successors(condition: u64);
    fn cleanup_explicit_or_sibling(condition: u64);
    fn cleanup_common_predecessor(condition: u64);
    fn cleanup_return_transfer(condition: u64) -> u64;
    fn cleanup_transparent_copy();
    fn cleanup_binding_alias();
    fn cleanup_box_alias();
    fn cleanup_unbox_alias();
    fn cleanup_loop_rearm();
    fn cleanup_return_borrowed(borrowed: u64) -> u64;
    fn cleanup_rebind_parameter(owner: u64, condition: u64) -> u64;
    fn cleanup_slot_snapshot_rebind() -> u64;
    fn cleanup_optional_rebind_return(owner: u64, condition: u64) -> u64;
    fn cleanup_explicit_credit();
    fn cleanup_conditional_credit(condition: u64);
    fn cleanup_iterator_multi_results(iterator: u64) -> u64;
    fn cleanup_local_result_store_snapshot() -> u64;
    fn cleanup_explicit_credit_rebind();
    fn cleanup_parameter_snapshot_rebind(owner: u64) -> u64;
    fn cleanup_mutable_source_result_store_snapshot() -> u64;
    fn cleanup_oracle_reset();
    fn cleanup_oracle_allocations() -> u64;
    fn cleanup_oracle_inc_refs() -> u64;
    fn cleanup_oracle_dec_refs() -> u64;
    fn cleanup_oracle_live_refs() -> u64;
    fn cleanup_oracle_owner(index: u64) -> u64;
    fn molt_classmethod_new(borrowed: u64) -> u64;
    fn molt_dec_ref_obj(value: u64);
}

const BOXED_FALSE: u64 = @BOXED_FALSE@;
const BOXED_TRUE: u64 = @BOXED_TRUE@;

unsafe fn assert_counts(label: &str, allocations: u64, incs: u64, decs: u64, live: u64) {
    assert_eq!(cleanup_oracle_allocations(), allocations, "{label}: allocations");
    assert_eq!(cleanup_oracle_inc_refs(), incs, "{label}: retains");
    assert_eq!(cleanup_oracle_dec_refs(), decs, "{label}: releases");
    assert_eq!(cleanup_oracle_live_refs(), live, "{label}: live references");
}

unsafe fn reset() { cleanup_oracle_reset(); }

fn main() {
    unsafe {
        reset();
        cleanup_dynamic_successors(BOXED_FALSE);
        assert_counts("dynamic false successor", 1, 0, 1, 0);
        reset();
        cleanup_dynamic_successors(BOXED_TRUE);
        assert_counts("dynamic true successor", 1, 0, 1, 0);

        reset();
        cleanup_explicit_or_sibling(BOXED_FALSE);
        assert_counts("explicit release branch", 1, 0, 1, 0);
        reset();
        cleanup_explicit_or_sibling(BOXED_TRUE);
        assert_counts("sibling cleanup branch", 1, 0, 1, 0);

        reset();
        cleanup_common_predecessor(BOXED_FALSE);
        assert_counts("common predecessor false", 1, 0, 1, 0);
        reset();
        cleanup_common_predecessor(BOXED_TRUE);
        assert_counts("common predecessor true", 1, 0, 1, 0);

        reset();
        let none = cleanup_return_transfer(BOXED_FALSE);
        assert_eq!(none, @BOXED_NONE@, "drop branch must return None");
        assert_counts("return sibling drop", 1, 0, 1, 0);
        reset();
        let transferred = cleanup_return_transfer(BOXED_TRUE);
        assert_counts("returned owner before caller release", 1, 0, 0, 1);
        molt_dec_ref_obj(transferred);
        assert_counts("returned owner after caller release", 1, 0, 1, 0);

        reset();
        cleanup_transparent_copy();
        assert_counts("transparent copy", 1, 0, 1, 0);
        reset();
        cleanup_binding_alias();
        assert_counts("binding alias", 1, 1, 2, 0);
        reset();
        cleanup_box_alias();
        assert_counts("box alias", 1, 1, 2, 0);
        reset();
        cleanup_unbox_alias();
        assert_counts("unbox alias", 1, 1, 2, 0);

        reset();
        cleanup_loop_rearm();
        assert_counts("two loop allocations", 2, 0, 2, 0);

        reset();
        let borrowed = molt_classmethod_new(BOXED_FALSE);
        let returned_borrow = cleanup_return_borrowed(borrowed);
        assert_eq!(returned_borrow, borrowed, "borrowed return must preserve object identity");
        assert_counts("borrowed return credit", 1, 1, 0, 2);
        molt_dec_ref_obj(returned_borrow);
        molt_dec_ref_obj(borrowed);
        assert_counts("borrowed return caller cleanup", 1, 1, 2, 0);

        reset();
        let original = molt_classmethod_new(BOXED_FALSE);
        let none = cleanup_rebind_parameter(original, BOXED_FALSE);
        assert_eq!(none, @BOXED_NONE@, "rebound drop path must return None");
        assert_counts("rebound parameter drop", 2, 0, 1, 1);
        molt_dec_ref_obj(original);
        assert_counts("rebound parameter drop caller cleanup", 2, 0, 2, 0);

        reset();
        let original = molt_classmethod_new(BOXED_FALSE);
        let returned_owner = cleanup_rebind_parameter(original, BOXED_TRUE);
        assert_ne!(returned_owner, original, "rebound return must transfer the fresh owner");
        assert_counts("rebound parameter return", 2, 0, 0, 2);
        molt_dec_ref_obj(returned_owner);
        molt_dec_ref_obj(original);
        assert_counts("rebound parameter return caller cleanup", 2, 0, 2, 0);

        reset();
        let snapshot = cleanup_slot_snapshot_rebind();
        assert_counts("slot snapshot before rebind", 2, 3, 4, 1);
        molt_dec_ref_obj(snapshot);
        assert_counts("slot snapshot caller cleanup", 2, 3, 5, 0);

        reset();
        let original = molt_classmethod_new(BOXED_FALSE);
        let returned_borrow = cleanup_optional_rebind_return(original, BOXED_FALSE);
        assert_eq!(returned_borrow, original, "non-rebound merge path must return the borrow");
        assert_counts("optional rebind borrowed merge", 1, 1, 0, 2);
        molt_dec_ref_obj(returned_borrow);
        molt_dec_ref_obj(original);
        assert_counts("optional rebind borrowed caller cleanup", 1, 1, 2, 0);

        reset();
        let original = molt_classmethod_new(BOXED_FALSE);
        let returned_owner = cleanup_optional_rebind_return(original, BOXED_TRUE);
        assert_ne!(returned_owner, original, "rebound merge path must transfer the fresh owner");
        assert_counts("optional rebind owned merge", 2, 0, 0, 2);
        molt_dec_ref_obj(returned_owner);
        molt_dec_ref_obj(original);
        assert_counts("optional rebind owned caller cleanup", 2, 0, 2, 0);

        reset();
        cleanup_explicit_credit();
        assert_counts("explicit retain credit then release", 1, 1, 2, 0);

        reset();
        cleanup_conditional_credit(BOXED_FALSE);
        assert_counts("conditional credit false merge", 1, 0, 1, 0);
        reset();
        cleanup_conditional_credit(BOXED_TRUE);
        assert_counts("conditional credit true merge", 1, 1, 2, 0);
        reset();

        let pair = molt_classmethod_new(BOXED_FALSE);
        let returned_pair = cleanup_iterator_multi_results(pair);
        assert_eq!(returned_pair, pair, "unpack must not substitute a key for the pair");
        assert_counts("iterator and trailing unpack result cleanup", 3, 1, 2, 2);
        molt_dec_ref_obj(returned_pair);
        molt_dec_ref_obj(pair);
        assert_counts("iterator result caller cleanup", 3, 1, 4, 0);
        reset();

        let snapshot = cleanup_local_result_store_snapshot();
        assert_eq!(snapshot, cleanup_oracle_owner(0), "local snapshot must survive the result-carrying rebind");
        assert_counts("local result store snapshot", 2, 3, 4, 1);
        molt_dec_ref_obj(snapshot);
        assert_counts("local result store caller cleanup", 2, 3, 5, 0);
        reset();

        cleanup_explicit_credit_rebind();
        assert_counts("explicit credit remains with old binding", 2, 2, 4, 0);
        reset();

        let original = molt_classmethod_new(BOXED_FALSE);
        let snapshot = cleanup_parameter_snapshot_rebind(original);
        assert_eq!(snapshot, original, "snapshot must refer to the original parameter binding");
        assert_counts("parameter snapshot before single rebind", 2, 1, 1, 2);
        molt_dec_ref_obj(snapshot);
        molt_dec_ref_obj(original);
        assert_counts("parameter snapshot caller cleanup", 2, 1, 3, 0);
        reset();

        let snapshot = cleanup_mutable_source_result_store_snapshot();
        assert_eq!(snapshot, cleanup_oracle_owner(0), "result store must snapshot a mutable source exactly once");
        assert_counts("mutable source result store snapshot", 2, 5, 6, 1);
        molt_dec_ref_obj(snapshot);
        assert_counts("mutable source result store caller cleanup", 2, 5, 7, 0);
        reset();
    }
}
"#
    .replace(
        "@BOXED_FALSE@",
        &(molt_codegen_abi::box_bool_bits(0) as u64).to_string(),
    )
    .replace(
        "@BOXED_TRUE@",
        &(molt_codegen_abi::box_bool_bits(1) as u64).to_string(),
    )
    .replace(
        "@BOXED_NONE@",
        &(molt_codegen_abi::box_none_bits() as u64).to_string(),
    );
    link_and_run_native_object(
        &rustc,
        "native-value-tracking-cleanup-oracle",
        object_bytes,
        &provider_source,
        &harness_source,
        "NativeValueTracking cleanup oracle",
    );
}

#[test]
fn native_direct_symbol_forward_f32_emits_relocation_and_bytes_bridge() {
    let symbol = "molt_native_forward_f32_probe";
    let ir = native_callable_program(
        "scipy.ndimage.distance_transform_edt",
        "direct_symbol",
        "molt.forward_f32_v1",
        Some(symbol),
        &["payload"],
    );

    let output = SimpleBackend::new().compile(ir);
    for expected in [
        symbol.as_bytes(),
        b"molt_bytes_as_ptr",
        b"molt_scratch_alloc",
        b"molt_scratch_free",
        b"molt_bytes_from",
    ] {
        assert!(
            object_contains(&output.bytes, expected),
            "native forward_f32 object is missing relocation {}",
            String::from_utf8_lossy(expected)
        );
    }
}

#[test]
fn native_direct_symbol_pyinit_emits_relocation() {
    let symbol = "PyInit__native_probe";
    let ir = native_callable_program(
        "native_probe._native_probe",
        "direct_symbol",
        "molt.pyinit_module_v1",
        Some(symbol),
        &[],
    );

    let output = SimpleBackend::new().compile(ir);
    assert!(object_contains(&output.bytes, symbol.as_bytes()));
}

#[test]
#[should_panic(expected = "molt.forward_f32_v1")]
fn native_direct_symbol_forward_f32_rejects_arity_drift() {
    let ir = native_callable_program(
        "native_probe.forward",
        "direct_symbol",
        "molt.forward_f32_v1",
        Some("molt_native_forward_f32_probe"),
        &["payload", "extra"],
    );
    let _ = SimpleBackend::new().compile(ir);
}

#[test]
#[should_panic(expected = "direct_symbol requires native_callable_symbol")]
fn native_direct_symbol_rejects_missing_symbol() {
    let ir = native_callable_program(
        "native_probe._native_probe",
        "direct_symbol",
        "molt.pyinit_module_v1",
        None,
        &[],
    );
    let _ = SimpleBackend::new().compile(ir);
}

#[test]
#[should_panic(expected = "invoke_ffi native_callable_symbol must be nonempty and printable")]
fn native_direct_symbol_rejects_empty_symbol() {
    let ir = native_callable_program(
        "native_probe._native_probe",
        "direct_symbol",
        "molt.pyinit_module_v1",
        Some(""),
        &[],
    );
    let _ = SimpleBackend::new().compile(ir);
}
