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
//!   2. direct-symbol / memory ABIs, which need a native import surface this
//!      dispatch does not provide, still FAIL CLOSED with a precise diagnostic
//!      (never a fake, fall-through, or silent no-op).

use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

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
            execution_context: Default::default(),
        }],
        profile: None,
    }
}

fn object_contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
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
#[should_panic(expected = "native ABI dispatch supports only `module_attr` object-call exports")]
fn native_direct_symbol_native_callable_fails_closed() {
    // direct_symbol binding needs a native import surface this dispatch does not
    // provide; it must fail closed with a precise diagnostic, never fall through
    // to a fake or a silent no-op (POISON guard).
    let ir = native_callable_program(
        "scipy.ndimage.distance_transform_edt",
        "direct_symbol",
        "molt.forward_f32_v1",
        Some("molt_scipy_ndimage_distance_transform_edt"),
        &["payload"],
    );
    let _ = SimpleBackend::new().compile(ir);
}
