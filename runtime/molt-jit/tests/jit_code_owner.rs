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
use std::sync::atomic::AtomicU32;

const SYMBOL: &str = "molt_native_object_call_execution_probe";
const SENTINEL: u64 = 0x45A1_7E57_D15C_A11E;
const ENTRY: &str = "native_callable_dispatch";

#[used]
static GENERATED_OBJECT_ABI: u8 = 0;

/// The thread record compiled code reads through its context, at the
/// generated offsets (`molt_codegen_abi::THREAD_RECORD_*`), as the AOT twin's
/// provider defines it: no exception pending, no pending invocation, a work
/// word with no work, no frame, an empty arena, zero admission, profile and
/// depth words, and the recursion limit's address.
#[repr(C)]
struct FixtureThreadRecord {
    exception_pending: u64,
    pending_invocation: u64,
    pending_work: &'static AtomicU32,
    words: [u64; 6],
    recursion_limit: &'static u64,
}
const _: () = {
    use std::mem::offset_of;
    assert!(
        offset_of!(FixtureThreadRecord, exception_pending)
            == molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as usize
    );
    assert!(
        offset_of!(FixtureThreadRecord, pending_invocation)
            == molt_codegen_abi::THREAD_RECORD_PENDING_INVOCATION_OFFSET as usize
    );
    assert!(
        offset_of!(FixtureThreadRecord, pending_work)
            == molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as usize
    );
    assert!(
        offset_of!(FixtureThreadRecord, recursion_limit)
            == molt_codegen_abi::THREAD_RECORD_RECURSION_LIMIT_PTR_OFFSET as usize
    );
};
static WORK_WORD: AtomicU32 = AtomicU32::new(0);
static RECURSION_LIMIT: u64 = 1000;
/// Compiled code writes the record (frame top, recursion depth), so it lives
/// in writable memory.
#[repr(transparent)]
struct Writable(std::cell::UnsafeCell<FixtureThreadRecord>);
// SAFETY: one test thread runs the compiled code at a time.
unsafe impl Sync for Writable {}
static THREAD_RECORD: Writable = Writable(std::cell::UnsafeCell::new(FixtureThreadRecord {
    exception_pending: 0,
    pending_invocation: 0,
    pending_work: &WORK_WORD,
    words: [0; 6],
    recursion_limit: &RECURSION_LIMIT,
}));

#[repr(C)]
struct FixtureContext {
    thread_record: &'static Writable,
}
static CONTEXT: FixtureContext = FixtureContext {
    thread_record: &THREAD_RECORD,
};

fn context() -> u64 {
    std::ptr::addr_of!(CONTEXT) as u64
}
fn assert_context(actual: u64) {
    assert_eq!(actual, context());
}

extern "C" fn dec_ref(ctx: u64, _: u64) {
    assert_context(ctx);
}
extern "C" fn inc_ref(ctx: u64, _: u64) {
    assert_context(ctx);
}
extern "C" fn exception_pending(ctx: u64) -> u64 {
    assert_context(ctx);
    0
}
extern "C" fn int_from_i64(ctx: u64, value: i64) -> u64 {
    assert_context(ctx);
    value as u64
}
extern "C" fn async_work_poll_and_exception_pending(ctx: u64) -> u64 {
    assert_context(ctx);
    0
}
/// The origin service of the probe call's exception edge, as the AOT twin's
/// provider archive defines it: the provider never sets an error, so no
/// Origin edge is taken.
extern "C" fn exception_frame_origin(ctx: u64, _homes: u64) -> u64 {
    assert_context(ctx);
    unreachable!("this provider never sets an error, so no Origin edge is taken")
}
extern "C" fn object_call_probe(_: u64) -> u64 {
    SENTINEL
}

/// The provider the AOT twin links as an archive, as name -> address.
fn provider() -> BTreeMap<&'static str, usize> {
    let physical = |logical| {
        molt_ir::runtime_callable_abi_generated::runtime_execution_abi(logical)
            .expect("fixture runtime import has a generated execution ABI")
            .physical_symbol
    };
    BTreeMap::from([
        (
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
            std::ptr::addr_of!(GENERATED_OBJECT_ABI) as usize,
        ),
        (
            physical("molt_dec_ref"),
            dec_ref as extern "C" fn(u64, u64) as usize,
        ),
        (
            physical("molt_dec_ref_obj"),
            dec_ref as extern "C" fn(u64, u64) as usize,
        ),
        (
            physical("molt_inc_ref_obj"),
            inc_ref as extern "C" fn(u64, u64) as usize,
        ),
        (
            physical("molt_exception_pending"),
            exception_pending as extern "C" fn(u64) -> u64 as usize,
        ),
        (
            physical("molt_int_from_i64"),
            int_from_i64 as extern "C" fn(u64, i64) -> u64 as usize,
        ),
        (
            physical("molt_async_work_poll_and_exception_pending"),
            async_work_poll_and_exception_pending as extern "C" fn(u64) -> u64 as usize,
        ),
        (
            physical("molt_exception_frame_origin"),
            exception_frame_origin as extern "C" fn(u64, u64) -> u64 as usize,
        ),
        (
            SYMBOL,
            object_call_probe as extern "C" fn(u64) -> u64 as usize,
        ),
    ])
}

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

/// `native_probe.execute(payload)` through a direct-symbol object call.
fn program() -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: ENTRY.to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    out: Some("payload".to_string()),
                    value: Some(1),
                    ..op(molt_ir::OpKind::Const)
                },
                OpIR {
                    out: Some("result".to_string()),
                    args: Some(vec!["payload".to_string()]),
                    native_callable_export: Some("native_probe.execute".to_string()),
                    native_callable_binding: Some("direct_symbol".to_string()),
                    native_callable_abi: Some("molt.object_call_v1".to_string()),
                    native_callable_symbol: Some(SYMBOL.to_string()),
                    ..op(molt_ir::OpKind::InvokeFfi)
                },
                OpIR {
                    args: Some(vec!["result".to_string()]),
                    ..op(molt_ir::OpKind::Ret)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
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
    // SAFETY: the compiled entry takes its live context before Python args.
    let entry: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(entry) };
    assert_eq!(entry(context()), SENTINEL);
}

#[test]
fn jit_refuses_a_required_callee_missing_from_the_document() {
    let mut ir = program();
    ir.functions[0].ops[1] = OpIR {
        s_value: Some("absent_provider".into()),
        out: Some("result".into()),
        args: Some(vec!["payload".into()]),
        ..op(molt_ir::OpKind::CallGuarded)
    };
    match backend(provider()).compile_jit(ir) {
        Err(JitCompileError::FunctionReferences(message)) => {
            assert!(
                message.contains(ENTRY) && message.contains("absent_provider"),
                "{message}"
            );
        }
        Err(other) => panic!("expected a missing function reference, got {other}"),
        Ok(_) => panic!("missing function resolved"),
    }
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
