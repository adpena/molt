//! The JIT code owner runs the code the object owner would link.
//!
//! These tests compile the program that
//! `native_direct_symbol_object_call_links_provider_archive_and_executes`
//! (molt-backend-native) links against a provider archive, but into process
//! memory, with the same provider symbols resolved through the JIT lookup.
//! They need no Molt runtime, so they prove the code owner, the relocation
//! model and the import closure check on their own.

use molt_backend_native::{FunctionIR, JitCompileError, OpIR, SimpleBackend, SimpleIR};
use std::collections::BTreeMap;
use std::sync::Arc;

const SYMBOL: &str = "molt_native_object_call_execution_probe";
const SENTINEL: u64 = 0x45A1_7E57_D15C_A11E;
const ENTRY: &str = "native_callable_dispatch";

#[used]
static GENERATED_OBJECT_ABI: u8 = 0;
static EXCEPTION_PENDING: u8 = 0;

extern "C" fn dec_ref(_: u64) {}
extern "C" fn inc_ref(_: u64) {}
extern "C" fn exception_pending_fast() -> u64 {
    0
}
extern "C" fn exception_pending_flag_ptr() -> u64 {
    std::ptr::addr_of!(EXCEPTION_PENDING) as u64
}
extern "C" fn int_from_i64(value: i64) -> u64 {
    value as u64
}
extern "C" fn async_work_poll_and_exception_pending() -> u64 {
    0
}
extern "C" fn object_call_probe(_: u64) -> u64 {
    SENTINEL
}

/// The provider the AOT twin links as an archive, as name -> address.
fn provider() -> BTreeMap<&'static str, usize> {
    BTreeMap::from([
        (
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
            std::ptr::addr_of!(GENERATED_OBJECT_ABI) as usize,
        ),
        ("molt_dec_ref", dec_ref as extern "C" fn(u64) as usize),
        ("molt_dec_ref_obj", dec_ref as extern "C" fn(u64) as usize),
        ("molt_inc_ref_obj", inc_ref as extern "C" fn(u64) as usize),
        (
            "molt_exception_pending_fast",
            exception_pending_fast as extern "C" fn() -> u64 as usize,
        ),
        (
            "molt_exception_pending_flag_ptr",
            exception_pending_flag_ptr as extern "C" fn() -> u64 as usize,
        ),
        (
            "molt_int_from_i64",
            int_from_i64 as extern "C" fn(i64) -> u64 as usize,
        ),
        (
            "molt_async_work_poll_and_exception_pending",
            async_work_poll_and_exception_pending as extern "C" fn() -> u64 as usize,
        ),
        (
            SYMBOL,
            object_call_probe as extern "C" fn(u64) -> u64 as usize,
        ),
    ])
}

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

/// `native_probe.execute(payload)` through a direct-symbol object call.
fn program() -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: ENTRY.to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("payload".to_string()),
                    value: Some(1),
                    ..op("const")
                },
                OpIR {
                    out: Some("result".to_string()),
                    args: Some(vec!["payload".to_string()]),
                    native_callable_export: Some("native_probe.execute".to_string()),
                    native_callable_binding: Some("direct_symbol".to_string()),
                    native_callable_abi: Some("molt.object_call_v1".to_string()),
                    native_callable_symbol: Some(SYMBOL.to_string()),
                    ..op("invoke_ffi")
                },
                OpIR {
                    args: Some(vec!["result".to_string()]),
                    ..op("ret")
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    }
}

fn backend(symbols: BTreeMap<&'static str, usize>) -> SimpleBackend {
    SimpleBackend::new_jit(Arc::new(move |name| {
        symbols.get(name).map(|&address| address as *const u8)
    }))
}

#[test]
fn executable_memory_probe_runs_generated_code() {
    molt_backend_native::probe_jit_execution().expect("host admits JIT code");
}

#[test]
fn jit_program_calls_its_imports_through_the_lookup() {
    let program = backend(provider())
        .compile_jit(program())
        .expect("compile the probe program into memory");
    let entry = program
        .function_address(ENTRY)
        .expect("the program defines its entry");
    // SAFETY: the entry was compiled as `extern "C" fn() -> u64`.
    let entry: extern "C" fn() -> u64 = unsafe { std::mem::transmute(entry) };
    assert_eq!(entry(), SENTINEL);
}

#[test]
fn an_import_the_runtime_lacks_fails_before_any_code_runs() {
    let mut symbols = provider();
    symbols.remove(SYMBOL);
    let error = match backend(symbols).compile_jit(program()) {
        Ok(_) => panic!("a missing import must refuse the program"),
        Err(error) => error,
    };
    match &error {
        JitCompileError::UnresolvedImports(names) => {
            assert_eq!(names, &vec![SYMBOL.to_string()]);
        }
        other => panic!("expected an unresolved import, got {other}"),
    }
    assert!(error.to_string().contains(SYMBOL), "{error}");
}

#[test]
fn the_abi_witness_is_an_import_like_any_other() {
    // A runtime built for the other execution mode lacks this exact witness;
    // the JIT refuses it as the AOT link does.
    let mut symbols = provider();
    symbols.remove(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL);
    match backend(symbols).compile_jit(program()) {
        Err(JitCompileError::UnresolvedImports(names)) => assert_eq!(
            names,
            vec![molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL.to_string()]
        ),
        Err(other) => panic!("expected an unresolved witness, got {other}"),
        Ok(_) => panic!("a missing ABI witness must refuse the program"),
    }
}

#[test]
fn jit_and_object_owners_differ_only_in_the_relocation_model() {
    // One ISA constructor serves both owners. Process memory takes long-range
    // libcalls everywhere and GOT-relative PIC on x86_64 only, as
    // cranelift_jit's own builder does; every other setting is shared.
    let object = SimpleBackend::object_codegen_identity(None);
    let jit = SimpleBackend::jit_codegen_identity();
    for field in ["target", "isa_flags", "tir_simd", "backend"] {
        assert_eq!(object[field], jit[field], "{field}");
    }
    let object_flags = object["shared_flags"].as_object().expect("object flags");
    let jit_flags = jit["shared_flags"].as_object().expect("jit flags");
    assert_eq!(
        object_flags.keys().collect::<Vec<_>>(),
        jit_flags.keys().collect::<Vec<_>>()
    );
    let differing: Vec<&str> = object_flags
        .iter()
        .filter(|(name, value)| jit_flags[name.as_str()] != **value)
        .map(|(name, _)| name.as_str())
        .collect();
    let jit_pic = cfg!(target_arch = "x86_64");
    let mut expected = vec![];
    if object_flags["is_pic"] != serde_json::json!(jit_pic.to_string()) {
        expected.push("is_pic");
    }
    expected.push("use_colocated_libcalls");
    assert_eq!(differing, expected);
    assert_eq!(jit_flags["use_colocated_libcalls"], "false");
    assert_eq!(jit_flags["is_pic"], serde_json::json!(jit_pic.to_string()));
}
