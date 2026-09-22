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
//!   3. malformed arity and missing-symbol inputs still fail closed;
//!   4. structured-data parsers ignore stale raw-literal metadata, release raw
//!      input boxes, preserve boxing failures, and consume every owned result.

use super::native_object_symbols;
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
            return_abi: molt_ir::FunctionReturnAbi::Value,
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
        return_abi: molt_ir::FunctionReturnAbi::Value,
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

fn native_parser_function(name: &str, kind: &str, out: Option<&str>) -> FunctionIR {
    let mut ops = vec![OpIR {
        kind: kind.into(),
        args: Some(vec!["payload".into()]),
        out: out.map(str::to_owned),
        ..OpIR::default()
    }];
    if let Some(out) = out.filter(|out| *out != "none") {
        ops.push(ret(out));
    } else {
        ops.push(cleanup_ret_void());
    }
    cleanup_oracle_function(
        name,
        &["payload", "payload_ptr", "payload_len"],
        Some(&["dyn", "dyn", "dyn"]),
        ops,
    )
}

fn native_raw_parser_function(name: &str, kind: &str) -> FunctionIR {
    let function = cleanup_oracle_function(
        name,
        &[],
        None,
        vec![
            OpIR {
                kind: "const_int".into(),
                out: Some("limb".into()),
                value: Some(1_i64 << 31),
                ..OpIR::default()
            },
            OpIR {
                kind: "checked_mul".into(),
                args: Some(vec!["limb".into(), "limb".into()]),
                var: Some("wide".into()),
                out: Some("overflow".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: kind.into(),
                args: Some(vec!["wide".into()]),
                out: Some("parsed".into()),
                ..OpIR::default()
            },
            ret("parsed"),
        ],
    );
    let plan = crate::representation_plan::ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    assert!(
        plan.is_full_deopt_int_name("wide"),
        "{kind}: raw parser input must exercise owned overflow boxing"
    );
    function
}

#[test]
fn native_parsers_use_object_inputs_and_consume_every_owned_result() {
    const TARGETS: &[&str] = &[
        "parser_json_bound",
        "parser_json_absent",
        "parser_json_none",
        "parser_msgpack_bound",
        "parser_msgpack_absent",
        "parser_msgpack_none",
        "parser_cbor_bound",
        "parser_cbor_absent",
        "parser_cbor_none",
        "parser_json_raw",
        "parser_msgpack_raw",
        "parser_cbor_raw",
    ];
    let mut functions = Vec::new();
    for (prefix, kind) in [
        ("parser_json", "json_parse"),
        ("parser_msgpack", "msgpack_parse"),
        ("parser_cbor", "cbor_parse"),
    ] {
        functions.push(native_parser_function(
            &format!("{prefix}_bound"),
            kind,
            Some("parsed"),
        ));
        functions.push(native_parser_function(
            &format!("{prefix}_absent"),
            kind,
            None,
        ));
        functions.push(native_parser_function(
            &format!("{prefix}_none"),
            kind,
            Some("none"),
        ));
        functions.push(native_raw_parser_function(&format!("{prefix}_raw"), kind));
    }

    let backend = compile_selected_functions_direct(functions, TARGETS);
    let object_bytes = emit_direct_object(backend);
    let imports = native_object_symbols(&object_bytes).undefined;
    for symbol in [
        "molt_json_parse_scalar_obj",
        "molt_msgpack_parse_scalar_obj",
        "molt_cbor_parse_scalar_obj",
        "molt_dec_ref_obj",
        "molt_exception_pending_fast",
        "molt_int_from_i64",
    ] {
        assert!(imports.contains(symbol), "missing parser import `{symbol}`");
    }
    for obsolete in [
        "molt_json_parse_scalar",
        "molt_msgpack_parse_scalar",
        "molt_cbor_parse_scalar",
    ] {
        assert!(
            !imports.contains(obsolete),
            "stale ptr/len metadata must not select `{obsolete}`"
        );
    }

    let Some(rustc) = real_rustc() else {
        return;
    };
    let provider_source = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering};

#[export_name = "@GENERATED_OBJECT_ABI_SYMBOL@"]
pub static GENERATED_OBJECT_ABI: u8 = 0;
static mut EXCEPTION_PENDING: u8 = 0;
static JSON_CALLS: AtomicU64 = AtomicU64::new(0);
static MSGPACK_CALLS: AtomicU64 = AtomicU64::new(0);
static CBOR_CALLS: AtomicU64 = AtomicU64::new(0);
static DEC_REFS: AtomicU64 = AtomicU64::new(0);
static LAST_RELEASE: AtomicU64 = AtomicU64::new(0);
static BOX_FAIL: AtomicU64 = AtomicU64::new(0);
static BOXES: AtomicU64 = AtomicU64::new(0);
static BOX_VALUE: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn molt_json_parse_scalar_obj(value: u64) -> u64 {
    JSON_CALLS.fetch_add(1, Ordering::SeqCst);
    value + 10
}
#[no_mangle]
pub extern "C" fn molt_msgpack_parse_scalar_obj(value: u64) -> u64 {
    MSGPACK_CALLS.fetch_add(1, Ordering::SeqCst);
    value + 20
}
#[no_mangle]
pub extern "C" fn molt_cbor_parse_scalar_obj(value: u64) -> u64 {
    CBOR_CALLS.fetch_add(1, Ordering::SeqCst);
    value + 30
}
#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(value: u64) {
    if value == @BOXED_NONE@ { return; }
    LAST_RELEASE.store(value, Ordering::SeqCst);
    DEC_REFS.fetch_add(1, Ordering::SeqCst);
}
#[no_mangle]
pub extern "C" fn molt_dec_ref(value: u64) { molt_dec_ref_obj(value); }
#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(_: u64) {}
#[no_mangle]
pub extern "C" fn parser_calls(codec: u64) -> u64 {
    match codec {
        0 => JSON_CALLS.load(Ordering::SeqCst),
        1 => MSGPACK_CALLS.load(Ordering::SeqCst),
        2 => CBOR_CALLS.load(Ordering::SeqCst),
        _ => 0,
    }
}
#[no_mangle]
pub extern "C" fn parser_dec_refs() -> u64 { DEC_REFS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn parser_last_release() -> u64 { LAST_RELEASE.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn parser_reset() {
    JSON_CALLS.store(0, Ordering::SeqCst);
    MSGPACK_CALLS.store(0, Ordering::SeqCst);
    CBOR_CALLS.store(0, Ordering::SeqCst);
    DEC_REFS.store(0, Ordering::SeqCst);
    LAST_RELEASE.store(0, Ordering::SeqCst);
    BOX_FAIL.store(0, Ordering::SeqCst);
    BOXES.store(0, Ordering::SeqCst);
    BOX_VALUE.store(0, Ordering::SeqCst);
    unsafe { EXCEPTION_PENDING = 0; }
}
#[no_mangle]
pub extern "C" fn parser_box_fail(fail: u64) { BOX_FAIL.store(fail, Ordering::SeqCst); }
#[no_mangle]
pub extern "C" fn parser_boxes() -> u64 { BOXES.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn parser_box_value() -> u64 { BOX_VALUE.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn molt_exception_pending_fast() -> u64 { unsafe { EXCEPTION_PENDING as u64 } }
#[no_mangle]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {
    core::ptr::addr_of!(EXCEPTION_PENDING) as u64
}
#[no_mangle]
pub extern "C" fn molt_async_work_poll_and_exception_pending() -> u64 {
    molt_exception_pending_fast()
}
#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    BOXES.fetch_add(1, Ordering::SeqCst);
    BOX_VALUE.store(value as u64, Ordering::SeqCst);
    if BOX_FAIL.load(Ordering::SeqCst) != 0 {
        unsafe { EXCEPTION_PENDING = 1; }
        @BOXED_NONE@
    } else {
        0x100
    }
}
"#
    .replace(
        "@GENERATED_OBJECT_ABI_SYMBOL@",
        molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
    )
    .replace(
        "@BOXED_NONE@",
        &(molt_codegen_abi::box_none_bits() as u64).to_string(),
    );
    let harness_source = r#"
const BOXED_NONE: u64 = @BOXED_NONE@;

extern "C" {
    fn parser_json_bound(payload: u64, payload_ptr: u64, payload_len: u64) -> u64;
    fn parser_json_absent(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_json_none(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_msgpack_bound(payload: u64, payload_ptr: u64, payload_len: u64) -> u64;
    fn parser_msgpack_absent(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_msgpack_none(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_cbor_bound(payload: u64, payload_ptr: u64, payload_len: u64) -> u64;
    fn parser_cbor_absent(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_cbor_none(payload: u64, payload_ptr: u64, payload_len: u64);
    fn parser_json_raw() -> u64;
    fn parser_msgpack_raw() -> u64;
    fn parser_cbor_raw() -> u64;
    fn parser_calls(codec: u64) -> u64;
    fn parser_dec_refs() -> u64;
    fn parser_last_release() -> u64;
    fn parser_reset();
    fn parser_box_fail(fail: u64);
    fn parser_boxes() -> u64;
    fn parser_box_value() -> u64;
    fn molt_exception_pending_fast() -> u64;
    fn molt_dec_ref_obj(value: u64);
}

fn main() {
    unsafe {
        let json = parser_json_bound(1, 1001, 2001);
        let msgpack = parser_msgpack_bound(2, 1002, 2002);
        let cbor = parser_cbor_bound(3, 1003, 2003);
        assert_eq!((json, msgpack, cbor), (11, 22, 33));
        assert_eq!(parser_dec_refs(), 0, "bound owners were released in the callee");

        parser_json_absent(1, 3001, 4001);
        parser_json_none(1, 5001, 6001);
        parser_msgpack_absent(2, 3002, 4002);
        parser_msgpack_none(2, 5002, 6002);
        parser_cbor_absent(3, 3003, 4003);
        parser_cbor_none(3, 5003, 6003);
        assert_eq!(parser_dec_refs(), 6, "discarded parser owners must be released once");
        assert_eq!(parser_last_release(), 33, "object provider result must reach the sink");
        assert_eq!((parser_calls(0), parser_calls(1), parser_calls(2)), (3, 3, 3));

        molt_dec_ref_obj(json);
        molt_dec_ref_obj(msgpack);
        molt_dec_ref_obj(cbor);
        assert_eq!(parser_dec_refs(), 9, "caller cleanup must own bound results");

        parser_reset();
        let raw_json = parser_json_raw();
        let raw_msgpack = parser_msgpack_raw();
        let raw_cbor = parser_cbor_raw();
        assert_eq!((raw_json, raw_msgpack, raw_cbor), (0x10a, 0x114, 0x11e));
        assert_eq!(parser_boxes(), 3, "each raw input must be boxed exactly once");
        assert_eq!(parser_box_value(), 1_u64 << 62, "full-width input was truncated");
        assert_eq!((parser_calls(0), parser_calls(1), parser_calls(2)), (1, 1, 1));
        assert_eq!(parser_dec_refs(), 3, "raw input owners must be released after calls");
        for result in [raw_json, raw_msgpack, raw_cbor] { molt_dec_ref_obj(result); }
        assert_eq!(parser_dec_refs(), 6, "caller must own successful parser results");

        for (codec, run) in [
            (0, parser_json_raw as unsafe extern "C" fn() -> u64),
            (1, parser_msgpack_raw as unsafe extern "C" fn() -> u64),
            (2, parser_cbor_raw as unsafe extern "C" fn() -> u64),
        ] {
            parser_reset();
            parser_box_fail(1);
            assert_eq!(run(), BOXED_NONE, "boxing failure must publish None");
            assert_eq!(parser_boxes(), 1, "boxing failure count");
            assert_eq!(parser_dec_refs(), 0, "failed boxing created no releasable owner");
            assert_eq!((parser_calls(0), parser_calls(1), parser_calls(2)), (0, 0, 0),
                "codec {codec}: provider ran after boxing failure");
            assert_eq!(molt_exception_pending_fast(), 1, "boxing exception was lost");
        }
    }
}
"#
    .replace(
        "@BOXED_NONE@",
        &(molt_codegen_abi::box_none_bits() as u64).to_string(),
    );
    link_and_run_native_object(
        &rustc,
        "native-parser-object-input",
        object_bytes,
        &provider_source,
        &harness_source,
        "native object-input parser",
    );
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

fn cleanup_binding_shape_function(
    name: &str,
    var: Option<&str>,
    out: Option<&str>,
    result_snapshot: bool,
    labelled: bool,
) -> FunctionIR {
    let mut function = cleanup_result_store_function(name, "first_owner", "second_owner");
    function.ops[2].var = var.map(str::to_string);
    function.ops[2].out = out.map(str::to_string);
    function.ops[5].out = Some("none".into());
    if result_snapshot {
        function.ops.pop();
        function.ops.push(ret("first_result"));
        function.ops.remove(3); // Return the store's actual result, not a later load.
    }
    if labelled {
        let boundary = if result_snapshot { 3 } else { 4 };
        function.ops.insert(boundary, cleanup_jump(130));
        function.ops.insert(boundary + 1, cleanup_label(130));
    }
    function
}

#[test]
#[should_panic(expected = "no codegen for binding op kind `store_fast`")]
fn native_unhandled_binding_without_out_fails_closed() {
    let function = cleanup_oracle_function(
        "unsupported_binding",
        &["source"],
        Some(&["dyn"]),
        vec![
            OpIR {
                kind: "store_fast".into(),
                var: Some("local".into()),
                args: Some(vec!["source".into()]),
                ..OpIR::default()
            },
            cleanup_ret_void(),
        ],
    );
    compile_selected_functions_direct(vec![function], &["unsupported_binding"]);
}

fn cleanup_integer_binding_function(
    name: &str,
    boxed_destination: bool,
    boxed_snapshot: bool,
    identity_result: bool,
) -> FunctionIR {
    let snapshot = "snapshot";
    let mut ops = vec![
        OpIR {
            kind: "const_int".into(),
            out: Some("lhs".into()),
            value: Some(1_i64 << 31),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_int".into(),
            out: Some("rhs".into()),
            value: Some(1_i64 << 31),
            ..OpIR::default()
        },
        OpIR {
            kind: "checked_mul".into(),
            args: Some(vec!["lhs".into(), "rhs".into()]),
            var: Some("wide".into()),
            out: Some("overflow".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_none".into(),
            out: Some("none_value".into()),
            ..OpIR::default()
        },
    ];
    if boxed_destination {
        ops.push(OpIR {
            kind: "store_var".into(),
            var: Some("local".into()),
            args: Some(vec!["none_value".into()]),
            ..OpIR::default()
        });
    }
    ops.push(OpIR {
        kind: "store_var".into(),
        var: Some("local".into()),
        out: Some(snapshot.into()),
        args: Some(vec!["wide".into()]),
        ..OpIR::default()
    });
    if identity_result {
        ops.extend([
            OpIR {
                kind: "load_var".into(),
                var: Some("local".into()),
                out: Some("loaded".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "is".into(),
                args: Some(vec!["loaded".into(), snapshot.into()]),
                out: Some("same_object".into()),
                ..OpIR::default()
            },
        ]);
    }
    if boxed_destination {
        ops.push(OpIR {
            kind: "store_var".into(),
            var: Some("local".into()),
            args: Some(vec!["none_value".into()]),
            ..OpIR::default()
        });
    }
    ops.push(ret(if identity_result {
        "same_object"
    } else {
        snapshot
    }));
    let function = cleanup_oracle_function(
        name,
        &[if boxed_snapshot { snapshot } else { "unused" }],
        Some(&["dyn"]),
        ops,
    );
    let plan = crate::representation_plan::ScalarRepresentationPlan::for_function_ir_for_target(
        &function,
        &crate::tir::TargetInfo::native_release_fast(),
    );
    assert!(
        plan.is_full_deopt_int_name("wide"),
        "full-width source: {name}"
    );
    assert_eq!(
        plan.is_full_deopt_int_name("local"),
        !boxed_destination,
        "destination: {name}"
    );
    assert_eq!(
        plan.is_full_deopt_int_name(snapshot),
        !boxed_snapshot,
        "snapshot: {name}"
    );
    function
}

#[test]
fn native_binding_emission_rejects_empty_and_reserved_destinations() {
    for destination in ["", "none"] {
        let function = cleanup_oracle_function(
            "invalid_binding",
            &["source"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "store_var".into(),
                    var: Some(destination.into()),
                    args: Some(vec!["source".into()]),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        );
        let error = std::panic::catch_unwind(|| {
            compile_selected_functions_direct(vec![function], &["invalid_binding"]);
        })
        .expect_err("reserved/empty names are not native storage");
        let message = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            message.contains("store_var requires a nonempty, non-reserved binding destination"),
            "{message}"
        );
    }
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
        "cleanup_binding_result_direct",
        "cleanup_binding_result_labelled",
        "cleanup_binding_out_only_direct",
        "cleanup_binding_out_only_labelled",
        "cleanup_binding_same_name_direct",
        "cleanup_binding_same_name_labelled",
        "cleanup_binding_none_result_direct",
        "cleanup_binding_none_result_labelled",
        "cleanup_binding_raw_raw",
        "cleanup_binding_boxed_raw",
        "cleanup_binding_raw_boxed",
        "cleanup_binding_boxed_boxed",
        "cleanup_binding_source_result",
        "cleanup_binding_shared_identity",
        "cleanup_load_argument_precedence",
        "cleanup_iterator_discard_value",
        "cleanup_iterator_discard_done",
        "cleanup_iterator_discard_both",
        "cleanup_checked_add_discard_value",
        "cleanup_checked_mul_discard_value",
        "cleanup_checked_raw_discard_positions",
        "cleanup_reserved_none_binding",
        "cleanup_reserved_none_load",
        "cleanup_reserved_none_return",
        "cleanup_iterator_pair_absent",
        "cleanup_iterator_pair_none",
        "cleanup_borrowed_call_bound",
        "cleanup_borrowed_call_absent",
        "cleanup_borrowed_call_none",
        "cleanup_dict_set_bound",
        "cleanup_dict_set_absent",
        "cleanup_dict_set_none",
        "cleanup_dict_update_missing_bound",
        "cleanup_dict_update_missing_absent",
        "cleanup_dict_update_missing_none",
        "cleanup_store_index_metadata",
        "cleanup_dict_set_temporary",
        "cleanup_hash_dict_bound",
        "cleanup_hash_dict_absent",
        "cleanup_hash_dict_none",
        "cleanup_hash_set_bound",
        "cleanup_hash_set_absent",
        "cleanup_hash_set_none",
        "cleanup_hash_frozenset_bound",
        "cleanup_hash_frozenset_absent",
        "cleanup_hash_frozenset_none",
        "cleanup_hash_dict_raw",
        "cleanup_hash_set_raw",
        "cleanup_hash_frozenset_raw",
        "cleanup_copy_join_metadata_absent",
        "cleanup_copy_join_metadata_empty",
        "cleanup_load_join_metadata_absent",
        "cleanup_load_join_metadata_empty",
        "cleanup_phi_join_metadata",
    ];
    let Some(rustc) = real_rustc() else {
        return;
    };

    let mut functions = vec![
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
        cleanup_binding_shape_function(
            TARGET_NAMES[20],
            Some("local"),
            Some("first_result"),
            true,
            false,
        ),
        cleanup_binding_shape_function(
            TARGET_NAMES[21],
            Some("local"),
            Some("first_result"),
            true,
            true,
        ),
        cleanup_binding_shape_function(TARGET_NAMES[22], None, Some("local"), false, false),
        cleanup_binding_shape_function(TARGET_NAMES[23], None, Some("local"), false, true),
        cleanup_binding_shape_function(
            TARGET_NAMES[24],
            Some("local"),
            Some("local"),
            false,
            false,
        ),
        cleanup_binding_shape_function(TARGET_NAMES[25], Some("local"), Some("local"), false, true),
        cleanup_binding_shape_function(TARGET_NAMES[26], Some("local"), Some("none"), false, false),
        cleanup_binding_shape_function(TARGET_NAMES[27], Some("local"), Some("none"), false, true),
        cleanup_integer_binding_function(TARGET_NAMES[28], false, false, false),
        cleanup_integer_binding_function(TARGET_NAMES[29], true, false, false),
        cleanup_integer_binding_function(TARGET_NAMES[30], false, true, false),
        cleanup_integer_binding_function(TARGET_NAMES[31], true, true, false),
        cleanup_oracle_function(
            TARGET_NAMES[32],
            &["owner"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "store_var".into(),
                    var: Some("local".into()),
                    out: Some("owner".into()),
                    args: Some(vec!["owner".into()]),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("owner", "owner"),
                OpIR {
                    kind: "load_var".into(),
                    var: Some("local".into()),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                ret("snapshot"),
            ],
        ),
        cleanup_integer_binding_function(TARGET_NAMES[33], true, true, true),
        cleanup_oracle_function(
            TARGET_NAMES[34],
            &["borrowed"],
            Some(&["dyn"]),
            vec![
                cleanup_classmethod_new("borrowed", "first_owner"),
                cleanup_classmethod_new("borrowed", "metadata_owner"),
                OpIR {
                    kind: "load_var".into(),
                    args: Some(vec!["first_owner".into()]),
                    var: Some("metadata_owner".into()),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                ret("snapshot"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[35],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next_unboxed".into(),
                    args: Some(vec!["iterator".into()]),
                    var: Some("none".into()),
                    out: Some("done".into()),
                    ..OpIR::default()
                },
                ret("done"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[36],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next_unboxed".into(),
                    args: Some(vec!["iterator".into()]),
                    var: Some("value".into()),
                    out: None,
                    ..OpIR::default()
                },
                ret("value"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[37],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next_unboxed".into(),
                    args: Some(vec!["iterator".into()]),
                    var: None,
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[38],
            &["lhs", "rhs"],
            Some(&["dyn", "dyn"]),
            vec![
                OpIR {
                    kind: "checked_add".into(),
                    args: Some(vec!["lhs".into(), "rhs".into()]),
                    var: Some("none".into()),
                    out: Some("overflow".into()),
                    ..OpIR::default()
                },
                ret("overflow"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[39],
            &["lhs", "rhs"],
            Some(&["dyn", "dyn"]),
            vec![
                OpIR {
                    kind: "checked_mul".into(),
                    args: Some(vec!["lhs".into(), "rhs".into()]),
                    var: None,
                    out: Some("overflow".into()),
                    ..OpIR::default()
                },
                ret("overflow"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[40],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_int".into(),
                    out: Some("limb".into()),
                    value: Some(1_i64 << 31),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "checked_mul".into(),
                    args: Some(vec!["limb".into(), "limb".into()]),
                    var: Some("wide".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "checked_add".into(),
                    args: Some(vec!["wide".into(), "wide".into()]),
                    var: Some("none".into()),
                    out: Some("overflow".into()),
                    ..OpIR::default()
                },
                ret("overflow"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[41],
            &[],
            None,
            vec![
                OpIR {
                    kind: "store_var".into(),
                    args: Some(vec!["none".into()]),
                    var: Some("local".into()),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                cleanup_classmethod_new("none", "owner"),
                OpIR {
                    kind: "store_var".into(),
                    args: Some(vec!["owner".into()]),
                    var: Some("local".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".into(),
                    args: Some(vec!["none".into()]),
                    var: Some("local".into()),
                    out: Some("cleared".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".into(),
                    args: Some(vec!["snapshot".into(), "cleared".into()]),
                    out: Some("both_none".into()),
                    ..OpIR::default()
                },
                ret("both_none"),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[42],
            &[],
            None,
            vec![
                OpIR {
                    kind: "load_var".into(),
                    var: Some("none".into()),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                ret("snapshot"),
            ],
        ),
        cleanup_oracle_function(TARGET_NAMES[43], &[], None, vec![ret("none")]),
        cleanup_oracle_function(
            TARGET_NAMES[44],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next".into(),
                    args: Some(vec!["iterator".into()]),
                    out: None,
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
        cleanup_oracle_function(
            TARGET_NAMES[45],
            &["iterator"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "iter_next".into(),
                    args: Some(vec!["iterator".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                cleanup_ret_void(),
            ],
        ),
    ];
    for (name, output) in [
        (TARGET_NAMES[46], Some("result")),
        (TARGET_NAMES[47], None),
        (TARGET_NAMES[48], Some("none")),
    ] {
        functions.push(cleanup_oracle_function(
            name,
            &["source"],
            Some(&["dyn"]),
            vec![
                OpIR {
                    kind: "call".into(),
                    args: Some(vec!["source".into(), "none".into(), "none".into()]),
                    out: output.map(str::to_string),
                    s_value: Some("molt_dict_set".into()),
                    ..OpIR::default()
                },
                if output == Some("result") {
                    ret("result")
                } else {
                    cleanup_ret_void()
                },
            ],
        ));
    }
    for (offset, kind) in [(49, "dict_set"), (52, "dict_update_missing")] {
        for (index, output) in [Some("result"), None, Some("none")].into_iter().enumerate() {
            functions.push(cleanup_oracle_function(
                TARGET_NAMES[offset + index],
                &["source"],
                Some(&["dyn"]),
                vec![
                    OpIR {
                        kind: kind.into(),
                        args: Some(vec!["source".into(), "none".into(), "none".into()]),
                        out: output.map(str::to_string),
                        ..OpIR::default()
                    },
                    if output == Some("result") {
                        ret("result")
                    } else {
                        cleanup_ret_void()
                    },
                ],
            ));
        }
    }
    functions.push(cleanup_oracle_function(
        TARGET_NAMES[55],
        &["source"],
        Some(&["dyn"]),
        vec![
            OpIR {
                kind: "store_index".into(),
                args: Some(vec!["source".into(), "none".into(), "none".into()]),
                out: Some("not_a_result".into()),
                ..OpIR::default()
            },
            cleanup_ret_void(),
        ],
    ));
    functions.push(cleanup_oracle_function(
        TARGET_NAMES[56],
        &[],
        None,
        vec![
            cleanup_classmethod_new("none", "source"),
            OpIR {
                kind: "dict_set".into(),
                args: Some(vec!["source".into(), "none".into(), "none".into()]),
                out: Some("result".into()),
                ..OpIR::default()
            },
            ret("result"),
        ],
    ));
    for (offset, kind, width) in [
        (57, "dict_new", 2),
        (60, "set_new", 1),
        (63, "frozenset_new", 1),
    ] {
        for (index, output) in [Some("result"), None, Some("none")].into_iter().enumerate() {
            let args = (0..3)
                .flat_map(|_| {
                    if width == 2 {
                        vec!["source".into(), "none".into()]
                    } else {
                        vec!["source".into()]
                    }
                })
                .collect();
            functions.push(cleanup_oracle_function(
                TARGET_NAMES[offset + index],
                &[],
                None,
                vec![
                    cleanup_classmethod_new("none", "source"),
                    OpIR {
                        kind: kind.into(),
                        args: Some(args),
                        out: output.map(str::to_string),
                        ..OpIR::default()
                    },
                    if output == Some("result") {
                        ret("result")
                    } else {
                        cleanup_ret_void()
                    },
                ],
            ));
        }
    }
    for (index, kind, width) in [
        (66, "dict_new", 2),
        (67, "set_new", 1),
        (68, "frozenset_new", 1),
    ] {
        let function = cleanup_oracle_function(
            TARGET_NAMES[index],
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_int".into(),
                    value: Some(1_i64 << 31),
                    out: Some("limb".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "checked_mul".into(),
                    args: Some(vec!["limb".into(), "limb".into()]),
                    var: Some("wide".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: kind.into(),
                    args: Some(vec!["wide".into(); width * 3]),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
                ret("result"),
            ],
        );
        let plan = crate::representation_plan::ScalarRepresentationPlan::for_function_ir_for_target(
            &function,
            &crate::tir::TargetInfo::native_release_fast(),
        );
        assert!(
            plan.is_full_deopt_int_name("wide"),
            "{kind}: physical temporary box witness"
        );
        functions.push(function);
    }
    for (index, kind, empty_args) in [
        (69, "copy_var", false),
        (70, "copy_var", true),
        (71, "load_var", false),
        (72, "load_var", true),
    ] {
        functions.push(cleanup_oracle_function(
            TARGET_NAMES[index],
            &["_bb1_arg0", "source"],
            Some(&["dyn", "dyn"]),
            vec![
                OpIR {
                    kind: "try_start".into(),
                    value: Some(80),
                    ..OpIR::default()
                },
                OpIR {
                    kind: kind.into(),
                    var: Some("_bb1_arg0".into()),
                    args: Some(vec!["source".into()]),
                    out: Some("snapshot".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    value: Some(80),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_pop".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: kind.into(),
                    var: Some("_bb1_arg0".into()),
                    args: empty_args.then(Vec::new),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
                ret("result"),
                cleanup_label(80),
                ret("_bb1_arg0"),
            ],
        ));
    }
    functions.push(cleanup_oracle_function(
        TARGET_NAMES[73],
        &["_bb1_arg0", "source"],
        Some(&["dyn", "dyn"]),
        vec![
            OpIR {
                kind: "if".into(),
                args: Some(vec!["_bb1_arg0".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".into(),
                args: Some(vec!["source".into(), "source".into()]),
                out: Some("merged".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".into(),
                var: Some("_bb1_arg0".into()),
                args: Some(vec!["source".into()]),
                out: Some("snapshot".into()),
                ..OpIR::default()
            },
            ret("_bb1_arg0"),
        ],
    ));
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
static mut EXCEPTION_PENDING: u8 = 0;
static HASH_FAIL_ALLOC: AtomicU64 = AtomicU64::new(0);
static HASH_FAIL_INSERT: AtomicU64 = AtomicU64::new(0);
static HASH_INSERTS: AtomicU64 = AtomicU64::new(0);
static HASH_ERROR: AtomicU64 = AtomicU64::new(0);
static HASH_FAIL_BOX: AtomicU64 = AtomicU64::new(0);
static HASH_BOXES: AtomicU64 = AtomicU64::new(0);
static mut TOKENS: [u64; MAX_OWNERS] = [0; MAX_OWNERS];
static mut REFS: [u64; MAX_OWNERS] = [0; MAX_OWNERS];
static mut INTEGERS: [i64; MAX_OWNERS] = [0; MAX_OWNERS];

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
    HASH_FAIL_ALLOC.store(0, Ordering::SeqCst);
    HASH_FAIL_INSERT.store(0, Ordering::SeqCst);
    HASH_INSERTS.store(0, Ordering::SeqCst);
    HASH_ERROR.store(0, Ordering::SeqCst);
    HASH_FAIL_BOX.store(0, Ordering::SeqCst);
    HASH_BOXES.store(0, Ordering::SeqCst);
    unsafe { EXCEPTION_PENDING = 0; }
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
// Providers model ABI owner credits and failure boundaries, not hash semantics.
fn hash_failure(code: u64) -> u64 {
    assert_eq!(HASH_ERROR.swap(code, Ordering::SeqCst), 0, "original exception overwritten");
    unsafe { EXCEPTION_PENDING = 1; }
    BOXED_NONE
}
fn hash_insert(value: u64) -> u64 {
    assert_eq!(unsafe { EXCEPTION_PENDING }, 0, "insert after failed predecessor");
    let index = owner_index(value).expect("insert requires allocated hash container");
    assert_ne!(unsafe { REFS[index] }, 0, "insert into released container");
    let insertion = HASH_INSERTS.fetch_add(1, Ordering::SeqCst) + 1;
    if HASH_FAIL_INSERT.load(Ordering::SeqCst) == insertion { hash_failure(100 + insertion) }
    else { value }
}
fn hash_new() -> u64 {
    if HASH_FAIL_ALLOC.load(Ordering::SeqCst) != 0 { hash_failure(71) }
    else { molt_classmethod_new(BOXED_NONE) }
}
#[no_mangle]
pub extern "C" fn cleanup_hash_mode(allocate: u64, insert: u64) {
    HASH_FAIL_ALLOC.store(allocate, Ordering::SeqCst);
    HASH_FAIL_INSERT.store(insert, Ordering::SeqCst);
}
#[no_mangle]
pub extern "C" fn cleanup_hash_inserts() -> u64 { HASH_INSERTS.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_hash_error() -> u64 { HASH_ERROR.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn cleanup_hash_box_fail(index: u64) { HASH_FAIL_BOX.store(index, Ordering::SeqCst); }
#[no_mangle]
pub extern "C" fn cleanup_hash_boxes() -> u64 { HASH_BOXES.load(Ordering::SeqCst) }
#[no_mangle]
pub extern "C" fn molt_dict_set(value: u64, _: u64, _: u64) -> u64 { hash_insert(value) }
#[no_mangle]
pub extern "C" fn molt_store_index(value: u64, _: u64, _: u64) -> u64 { value }
#[no_mangle]
pub extern "C" fn molt_dict_update_missing(value: u64, _: u64, _: u64) -> u64 { value }
#[no_mangle]
pub extern "C" fn molt_dict_new(_: u64) -> u64 { hash_new() }
#[no_mangle]
pub extern "C" fn molt_set_new(_: u64) -> u64 { hash_new() }
#[no_mangle]
pub extern "C" fn molt_frozenset_new(_: u64) -> u64 { hash_new() }
#[no_mangle]
pub extern "C" fn molt_set_add(value: u64, _: u64) -> u64 { hash_insert(value); BOXED_NONE }
#[no_mangle]
pub extern "C" fn molt_frozenset_add(value: u64, _: u64) -> u64 { hash_insert(value); BOXED_NONE }
#[no_mangle]
pub extern "C" fn molt_recursion_enter_fast() -> u64 { 1 }
#[no_mangle]
pub extern "C" fn molt_recursion_exit_fast() {}
#[no_mangle]
pub extern "C" fn molt_raise_recursion_error() -> u64 { panic!("unexpected recursion failure") }
// These providers model only the owned-result ABI, not iterator semantics.
// The input token represents the materialized pair and must retain its identity.
#[no_mangle]
pub unsafe extern "C" fn molt_iter_next_unboxed(pair: u64, output: u64) -> u64 {
    molt_inc_ref_obj(pair);
    *(output as *mut u64) = pair;
    BOXED_FALSE
}
#[no_mangle]
pub extern "C" fn molt_iter_next(iterator: u64) -> u64 { molt_classmethod_new(iterator) }
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
pub extern "C" fn molt_exception_pending_fast() -> u64 { unsafe { EXCEPTION_PENDING as u64 } }
#[no_mangle]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {
    core::ptr::addr_of!(EXCEPTION_PENDING) as u64
}
#[no_mangle]
pub extern "C" fn molt_async_work_poll_and_exception_pending() -> u64 { molt_exception_pending_fast() }
#[no_mangle]
pub extern "C" fn molt_exception_pop() -> u64 { BOXED_NONE }
#[no_mangle]
pub extern "C" fn molt_is(lhs: u64, rhs: u64) -> u64 {
    for value in [lhs, rhs] {
        if let Some(index) = owner_index(value) {
            assert_ne!(unsafe { REFS[index] }, 0, "identity read after final release");
        }
    }
    // Runtime identity compares object bits, not equal integer payloads. It
    // borrows both operands and returns an immediate Boolean without credits.
    if lhs == rhs { BOXED_TRUE } else { BOXED_FALSE }
}
#[no_mangle]
pub extern "C" fn molt_is_truthy(value: u64) -> u64 {
    u64::from(value != BOXED_FALSE)
}
#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    if (-(1_i64 << 46)..(1_i64 << 46)).contains(&value) {
        INT_TAG | ((value as u64) & INT_MASK)
    } else {
        let boxing = HASH_BOXES.fetch_add(1, Ordering::SeqCst) + 1;
        if HASH_FAIL_BOX.load(Ordering::SeqCst) == boxing { return hash_failure(200 + boxing); }
        let token = molt_classmethod_new(BOXED_NONE);
        unsafe { INTEGERS[owner_index(token).unwrap()] = value; }
        token
    }
}
#[no_mangle]
pub extern "C" fn cleanup_oracle_integer(token: u64) -> i64 {
    let index = owner_index(token).expect("full-width integer must be heap boxed");
    unsafe {
        assert_ne!(REFS[index], 0, "read after final release");
        INTEGERS[index]
    }
}
#[no_mangle]
pub extern "C" fn molt_lt(lhs: u64, rhs: u64) -> u64 {
    if (lhs & INT_MASK) < (rhs & INT_MASK) { BOXED_TRUE } else { BOXED_FALSE }
}
#[no_mangle]
pub extern "C" fn molt_add(lhs: u64, rhs: u64) -> u64 {
    molt_int_from_i64(integer_value(lhs) + integer_value(rhs))
}
#[no_mangle]
pub extern "C" fn molt_mul(lhs: u64, rhs: u64) -> u64 {
    molt_int_from_i64(integer_value(lhs) * integer_value(rhs))
}
fn integer_value(bits: u64) -> i64 {
    if let Some(index) = owner_index(bits) {
        unsafe { assert_ne!(REFS[index], 0); INTEGERS[index] }
    } else {
        let payload = bits & INT_MASK;
        if payload & ((INT_MASK + 1) >> 1) != 0 {
            (payload as i64) - ((INT_MASK + 1) as i64)
        } else {
            payload as i64
        }
    }
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
    fn cleanup_binding_result_direct() -> u64;
    fn cleanup_binding_result_labelled() -> u64;
    fn cleanup_binding_out_only_direct() -> u64;
    fn cleanup_binding_out_only_labelled() -> u64;
    fn cleanup_binding_same_name_direct() -> u64;
    fn cleanup_binding_same_name_labelled() -> u64;
    fn cleanup_binding_none_result_direct() -> u64;
    fn cleanup_binding_none_result_labelled() -> u64;
    fn cleanup_binding_raw_raw(unused: u64) -> u64;
    fn cleanup_binding_boxed_raw(unused: u64) -> u64;
    fn cleanup_binding_raw_boxed(snapshot: u64) -> u64;
    fn cleanup_binding_boxed_boxed(snapshot: u64) -> u64;
    fn cleanup_binding_source_result(unused: u64) -> u64;
    fn cleanup_binding_shared_identity(snapshot: u64) -> u64;
    fn cleanup_load_argument_precedence(borrowed: u64) -> u64;
    fn cleanup_copy_join_metadata_absent(original: u64, source: u64) -> u64;
    fn cleanup_copy_join_metadata_empty(original: u64, source: u64) -> u64;
    fn cleanup_load_join_metadata_absent(original: u64, source: u64) -> u64;
    fn cleanup_load_join_metadata_empty(original: u64, source: u64) -> u64;
    fn cleanup_phi_join_metadata(original: u64, source: u64) -> u64;
    fn cleanup_iterator_discard_value(iterator: u64) -> u64;
    fn cleanup_iterator_discard_done(iterator: u64) -> u64;
    fn cleanup_iterator_discard_both(iterator: u64);
    fn cleanup_checked_add_discard_value(lhs: u64, rhs: u64) -> u64;
    fn cleanup_checked_mul_discard_value(lhs: u64, rhs: u64) -> u64;
    fn cleanup_checked_raw_discard_positions() -> u64;
    fn cleanup_reserved_none_binding() -> u64;
    fn cleanup_reserved_none_load() -> u64;
    fn cleanup_reserved_none_return() -> u64;
    fn cleanup_iterator_pair_absent(iterator: u64);
    fn cleanup_iterator_pair_none(iterator: u64);
    fn cleanup_borrowed_call_bound(source: u64) -> u64;
    fn cleanup_borrowed_call_absent(source: u64);
    fn cleanup_borrowed_call_none(source: u64);
    fn cleanup_dict_set_bound(source: u64) -> u64;
    fn cleanup_dict_set_absent(source: u64);
    fn cleanup_dict_set_none(source: u64);
    fn cleanup_dict_update_missing_bound(source: u64) -> u64;
    fn cleanup_dict_update_missing_absent(source: u64);
    fn cleanup_dict_update_missing_none(source: u64);
    fn cleanup_store_index_metadata(source: u64);
    fn cleanup_dict_set_temporary() -> u64;
    fn cleanup_hash_dict_bound() -> u64;
    fn cleanup_hash_dict_absent();
    fn cleanup_hash_dict_none();
    fn cleanup_hash_set_bound() -> u64;
    fn cleanup_hash_set_absent();
    fn cleanup_hash_set_none();
    fn cleanup_hash_frozenset_bound() -> u64;
    fn cleanup_hash_frozenset_absent();
    fn cleanup_hash_frozenset_none();
    fn cleanup_hash_dict_raw() -> u64;
    fn cleanup_hash_set_raw() -> u64;
    fn cleanup_hash_frozenset_raw() -> u64;
    fn cleanup_hash_mode(allocate: u64, insert: u64);
    fn cleanup_hash_inserts() -> u64;
    fn cleanup_hash_error() -> u64;
    fn cleanup_hash_box_fail(index: u64);
    fn cleanup_hash_boxes() -> u64;
    fn molt_exception_pending_fast() -> u64;
    fn cleanup_oracle_reset();
    fn cleanup_oracle_allocations() -> u64;
    fn cleanup_oracle_inc_refs() -> u64;
    fn cleanup_oracle_dec_refs() -> u64;
    fn cleanup_oracle_live_refs() -> u64;
    fn cleanup_oracle_owner(index: u64) -> u64;
    fn cleanup_oracle_integer(token: u64) -> i64;
    fn molt_classmethod_new(borrowed: u64) -> u64;
    fn molt_int_from_i64(value: i64) -> u64;
    fn molt_dec_ref_obj(value: u64);
}

const BOXED_FALSE: u64 = @BOXED_FALSE@;
const BOXED_TRUE: u64 = @BOXED_TRUE@;
const BOXED_NONE: u64 = @BOXED_NONE@;

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

        for (label, run, incs, decs) in [
            ("binding result direct", cleanup_binding_result_direct as unsafe extern "C" fn() -> u64, 2, 3),
            ("binding result labelled", cleanup_binding_result_labelled as unsafe extern "C" fn() -> u64, 2, 3),
            ("binding-only out direct", cleanup_binding_out_only_direct as unsafe extern "C" fn() -> u64, 3, 4),
            ("binding-only out labelled", cleanup_binding_out_only_labelled as unsafe extern "C" fn() -> u64, 3, 4),
            ("same binding/result direct", cleanup_binding_same_name_direct as unsafe extern "C" fn() -> u64, 3, 4),
            ("same binding/result labelled", cleanup_binding_same_name_labelled as unsafe extern "C" fn() -> u64, 3, 4),
            ("none result direct", cleanup_binding_none_result_direct as unsafe extern "C" fn() -> u64, 3, 4),
            ("none result labelled", cleanup_binding_none_result_labelled as unsafe extern "C" fn() -> u64, 3, 4),
        ] {
            let snapshot = run();
            assert_eq!(snapshot, cleanup_oracle_owner(0), "{label}: snapshot survives local rebind");
            assert_counts(label, 2, incs, decs, 1);
            molt_dec_ref_obj(snapshot);
            assert_counts(label, 2, incs, decs + 1, 0);
            reset();
        }
        for (label, run, allocations, incs, decs) in [
            ("raw/raw binding", cleanup_binding_raw_raw as unsafe extern "C" fn(u64) -> u64, 1, 0, 0),
            ("boxed/raw binding", cleanup_binding_boxed_raw as unsafe extern "C" fn(u64) -> u64, 2, 0, 1),
            ("raw/boxed binding", cleanup_binding_raw_boxed as unsafe extern "C" fn(u64) -> u64, 1, 0, 0),
            ("boxed/boxed binding", cleanup_binding_boxed_boxed as unsafe extern "C" fn(u64) -> u64, 1, 1, 1),
        ] {
            let result = run(BOXED_FALSE);
            assert_eq!(cleanup_oracle_integer(result), 1_i64 << 62, "{label}: full-width value");
            assert_counts(label, allocations, incs, decs, 1);
            molt_dec_ref_obj(result);
            assert_counts(label, allocations, incs, decs + 1, 0);
            reset();
        }
        assert_eq!(cleanup_binding_shared_identity(BOXED_FALSE), BOXED_TRUE);
        assert_counts("shared materialization identity", 1, 2, 3, 0);
        reset();
        let original = molt_classmethod_new(BOXED_FALSE);
        let result = cleanup_binding_source_result(original);
        assert_eq!(result, original, "source/result overlap must preserve incoming binding");
        assert_counts("source is snapshot", 2, 2, 2, 2);
        molt_dec_ref_obj(result);
        molt_dec_ref_obj(original);
        assert_counts("source is snapshot caller cleanup", 2, 2, 4, 0);
        reset();
        let result = cleanup_load_argument_precedence(BOXED_FALSE);
        assert_eq!(result, cleanup_oracle_owner(0), "load argument must win over var metadata");
        assert_counts("load argument precedence", 2, 0, 1, 1);
        molt_dec_ref_obj(result);
        assert_counts("load argument precedence caller cleanup", 2, 0, 2, 0);
        reset();
        for run in [
            cleanup_copy_join_metadata_absent as unsafe extern "C" fn(u64, u64) -> u64,
            cleanup_copy_join_metadata_empty,
            cleanup_load_join_metadata_absent,
            cleanup_load_join_metadata_empty,
        ] {
            let original = molt_classmethod_new(BOXED_FALSE);
            let source = molt_classmethod_new(BOXED_TRUE);
            let result = run(original, source);
            assert_eq!(result, original, "join-shaped var metadata must not replace the parameter's storage");
            assert_counts("join metadata preserves borrowed parameters", 2, 1, 0, 3);
            molt_dec_ref_obj(result);
            molt_dec_ref_obj(original);
            molt_dec_ref_obj(source);
            assert_counts("join metadata caller cleanup", 2, 1, 3, 0);
            reset();
        }
        assert_eq!(cleanup_phi_join_metadata(BOXED_TRUE, BOXED_FALSE), BOXED_TRUE,
            "phi join discovery must not rebind a load's metadata name on the true edge");
        assert_eq!(cleanup_phi_join_metadata(BOXED_FALSE, BOXED_TRUE), BOXED_FALSE,
            "phi join discovery must not rebind a load's metadata name on the false edge");
        assert_counts("phi metadata preserves scalar parameters", 0, 0, 0, 0);
        reset();
        let iterator = molt_classmethod_new(BOXED_FALSE);
        assert_eq!(cleanup_iterator_discard_value(iterator), BOXED_FALSE);
        assert_counts("discard iterator value", 1, 1, 1, 1);
        let value = cleanup_iterator_discard_done(iterator);
        assert_eq!(value, iterator);
        assert_counts("discard iterator done", 1, 2, 1, 2);
        molt_dec_ref_obj(value);
        cleanup_iterator_discard_both(iterator);
        assert_counts("discard both iterator results", 1, 3, 3, 1);
        molt_dec_ref_obj(iterator);
        assert_counts("discard iterator caller cleanup", 1, 3, 4, 0);
        reset();
        for (label, run) in [
            ("discard boxed checked sum", cleanup_checked_add_discard_value as unsafe extern "C" fn(u64, u64) -> u64),
            ("discard boxed checked product", cleanup_checked_mul_discard_value as unsafe extern "C" fn(u64, u64) -> u64),
        ] {
            let wide = molt_int_from_i64(1_i64 << 50);
            let one = molt_int_from_i64(1);
            assert_eq!(run(wide, one), BOXED_FALSE, "{label}: surviving overflow flag");
            assert_counts(label, 2, 0, 1, 1);
            molt_dec_ref_obj(wide);
            assert_counts(label, 2, 0, 2, 0);
            reset();
        }
        assert_eq!(cleanup_checked_raw_discard_positions(), BOXED_TRUE);
        assert_counts("discard raw checked positions", 0, 0, 0, 0);
        reset();
        assert_eq!(cleanup_reserved_none_binding(), BOXED_TRUE);
        assert_counts("reserved None binding and rebind", 1, 1, 2, 0);
        reset();
        assert_eq!(cleanup_reserved_none_load(), BOXED_NONE);
        assert_eq!(cleanup_reserved_none_return(), BOXED_NONE);
        assert_counts("reserved None load and return", 0, 0, 0, 0);
        reset();
        cleanup_iterator_pair_absent(BOXED_FALSE);
        assert_counts("absent iterator pair result", 1, 0, 1, 0);
        reset();
        cleanup_iterator_pair_none(BOXED_FALSE);
        assert_counts("reserved-none iterator pair result", 1, 0, 1, 0);
        reset();
        let source = molt_classmethod_new(BOXED_NONE);
        let result = cleanup_borrowed_call_bound(source);
        assert_eq!(result, source, "borrowed call preserves object identity");
        assert_counts("bound borrowed runtime result", 1, 1, 0, 2);
        molt_dec_ref_obj(result);
        cleanup_borrowed_call_absent(source);
        cleanup_borrowed_call_none(source);
        assert_counts("discarded borrowed runtime result", 1, 1, 1, 1);
        molt_dec_ref_obj(source);
        assert_counts("borrowed runtime caller cleanup", 1, 1, 2, 0);
        reset();
        let source = molt_classmethod_new(BOXED_NONE);
        let first = cleanup_dict_set_bound(source);
        let second = cleanup_dict_update_missing_bound(source);
        assert_eq!(first, source);
        assert_eq!(second, source);
        assert_counts("handwritten borrowed result publication", 1, 2, 0, 3);
        molt_dec_ref_obj(first);
        molt_dec_ref_obj(second);
        cleanup_dict_set_absent(source);
        cleanup_dict_set_none(source);
        cleanup_dict_update_missing_absent(source);
        cleanup_dict_update_missing_none(source);
        cleanup_store_index_metadata(source);
        assert_counts("handwritten borrowed discard and out metadata", 1, 2, 2, 1);
        molt_dec_ref_obj(source);
        assert_counts("handwritten borrowed caller cleanup", 1, 2, 3, 0);
        reset();
        let result = cleanup_dict_set_temporary();
        assert_counts("borrowed alias retained before source cleanup", 1, 1, 1, 1);
        molt_dec_ref_obj(result);
        assert_counts("borrowed alias caller cleanup", 1, 1, 2, 0);
        reset();
        for (label, run, width) in [
            ("dict raw", cleanup_hash_dict_raw as unsafe extern "C" fn() -> u64, 2),
            ("set raw", cleanup_hash_set_raw as unsafe extern "C" fn() -> u64, 1),
            ("frozenset raw", cleanup_hash_frozenset_raw as unsafe extern "C" fn() -> u64, 1),
        ] {
            for (allocate, insert, boxing) in [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 2, 0), (0, 0, 1), (0, 0, 2)] {
                cleanup_hash_mode(allocate, insert);
                cleanup_hash_box_fail(boxing);
                let result = run();
                let calls = if allocate != 0 { 0 } else if boxing != 0 { (boxing - 1) / width } else if insert != 0 { insert } else { 3 };
                let boxes = if allocate != 0 { 0 } else if boxing != 0 { boxing } else { calls * width };
                let allocations = if allocate != 0 { 0 } else { 1 + boxes - u64::from(boxing != 0) };
                let error = if allocate != 0 { 71 } else if boxing != 0 { 200 + boxing } else if insert != 0 { 100 + insert } else { 0 };
                if error == 0 {
                    assert_eq!(result, cleanup_oracle_owner(0), "{label}: aggregate identity");
                    assert_counts(label, allocations, 0, allocations - 1, 1);
                    molt_dec_ref_obj(result);
                } else {
                    assert_eq!(result, BOXED_NONE, "{label}: failed temporary publication");
                }
                assert_counts(label, allocations, 0, allocations, 0);
                assert_eq!(cleanup_hash_inserts(), calls, "{label}: insert boundary");
                assert_eq!(cleanup_hash_boxes(), boxes, "{label}: skipped later materialization");
                assert_eq!(cleanup_hash_error(), error, "{label}: original exception");
                assert_eq!(molt_exception_pending_fast(), u64::from(error != 0));
                reset();
            }
        }
        for (label, bound, absent, none) in [
            ("dict", cleanup_hash_dict_bound as unsafe extern "C" fn() -> u64,
                cleanup_hash_dict_absent as unsafe extern "C" fn(), cleanup_hash_dict_none as unsafe extern "C" fn()),
            ("set", cleanup_hash_set_bound as unsafe extern "C" fn() -> u64,
                cleanup_hash_set_absent as unsafe extern "C" fn(), cleanup_hash_set_none as unsafe extern "C" fn()),
            ("frozenset", cleanup_hash_frozenset_bound as unsafe extern "C" fn() -> u64,
                cleanup_hash_frozenset_absent as unsafe extern "C" fn(), cleanup_hash_frozenset_none as unsafe extern "C" fn()),
        ] {
            for (allocate, insert, expected_calls, error) in [(0, 0, 3, 0), (1, 0, 0, 71), (0, 1, 1, 101), (0, 2, 2, 102)] {
                let allocations = if allocate != 0 { 1 } else { 2 };
                cleanup_hash_mode(allocate, insert);
                let result = bound();
                if error == 0 {
                    assert_eq!(result, cleanup_oracle_owner(1), "{label}: published original owner");
                    assert_counts(label, allocations, 0, 1, 1);
                    molt_dec_ref_obj(result);
                } else {
                    assert_eq!(result, BOXED_NONE, "{label}: failure published partial container");
                }
                assert_counts(label, allocations, 0, allocations, 0);
                assert_eq!(cleanup_hash_inserts(), expected_calls, "{label}: skipped later inserts");
                assert_eq!(cleanup_hash_error(), error, "{label}: original exception");
                assert_eq!(molt_exception_pending_fast(), u64::from(error != 0));
                reset();
                for discarded in [absent, none] {
                    cleanup_hash_mode(allocate, insert);
                    discarded();
                    assert_counts(label, allocations, 0, allocations, 0);
                    assert_eq!(cleanup_hash_inserts(), expected_calls, "{label}: discarded later inserts");
                    assert_eq!(cleanup_hash_error(), error, "{label}: discarded original exception");
                    assert_eq!(molt_exception_pending_fast(), u64::from(error != 0));
                    reset();
                }
            }
        }
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
