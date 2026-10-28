use super::*;
use inkwell::context::Context;
use object::{Object, ObjectSection, ObjectSymbol};

// LLVM emits in process; the shared helper also owns subprocess arguments.
#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[test]
fn test_backend_module_name() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test_module");
    let ir = backend.dump_ir();
    assert!(
        ir.contains("test_module"),
        "IR should contain the module name, got: {ir}"
    );
}

#[test]
fn direct_object_emitter_imports_exact_generated_object_abi_witness() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "direct_object_abi");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-object-abi")
        .expect("create LLVM object outputs within Cargo image custody");
    let artifact = artifacts.path().join(if cfg!(windows) {
        "backend.obj"
    } else {
        "backend.o"
    });
    backend
        .emit_object(&artifact, MoltOptLevel::None)
        .expect("direct LLVM object emission");
    let bytes = std::fs::read(&artifact).expect("read direct LLVM object");
    let object = object::File::parse(bytes.as_slice()).expect("parse direct LLVM object");
    let pointer_bytes = if object.is_64() { 8 } else { 4 };
    let anchor = object
        .symbols()
        .find(|symbol| {
            symbol.name().is_ok_and(|name| {
                name == crate::GENERATED_OBJECT_ABI_ANCHOR_SYMBOL
                    || (object.format() == object::BinaryFormat::MachO
                        && name.strip_prefix('_')
                            == Some(crate::GENERATED_OBJECT_ABI_ANCHOR_SYMBOL))
            })
        })
        .expect("LLVM generated-object ABI anchor");
    let section = object
        .section_by_index(anchor.section_index().expect("LLVM anchor section"))
        .expect("read LLVM anchor section");
    assert!(section.align() >= pointer_bytes);
    assert_eq!((anchor.address() - section.address()) % pointer_bytes, 0);
    let undefined = crate::test_support::native_object_symbols(&bytes).undefined;
    assert!(
        undefined.contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL),
        "direct LLVM object must import exact selected ABI witness: {undefined:?}"
    );
    let opposite = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
        molt_codegen_abi::GENERATED_OBJECT_ABI_GIL_SYMBOL
    } else {
        molt_codegen_abi::GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
    };
    assert!(!undefined.contains(opposite));
}

#[test]
fn test_create_target_machine_all_levels() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "tm_test");
    assert_eq!(
        LlvmBackend::reloc_mode(),
        inkwell::targets::RelocMode::PIC,
        "LLVM object emission must stay PIC-compatible for the shared runtime link path"
    );
    // Simply ensure no panic at any opt level.
    let _tm_none = backend.create_target_machine(&MoltOptLevel::None);
    let _tm_speed = backend.create_target_machine(&MoltOptLevel::Speed);
    let _tm_agg = backend.create_target_machine(&MoltOptLevel::Aggressive);
}

#[test]
fn test_optimize_empty_module_smoke() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "opt_smoke");
    // Running passes on an empty module must not panic or error.
    backend
        .optimize(MoltOptLevel::Speed)
        .expect("empty-module optimization should succeed");
}

#[test]
fn test_optimize_invalid_pipeline_fails_closed_and_restores_linkage() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "opt_fail_closed");
    let i64_ty = ctx.i64_type();
    let func = backend.module.add_function(
        "visible_entry",
        i64_ty.fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let entry = ctx.append_basic_block(func, "entry");
    backend.builder.position_at_end(entry);
    backend
        .builder
        .build_return(Some(&i64_ty.const_zero()))
        .unwrap();

    let err = backend
        .run_optimization_passes(&MoltOptLevel::Speed, "not-a-real-pass")
        .expect_err("invalid LLVM pass pipeline must fail closed");
    assert!(
        err.contains("not-a-real-pass"),
        "error should identify the rejected pass pipeline: {err}"
    );
    assert_eq!(
        func.get_linkage(),
        inkwell::module::Linkage::External,
        "temporary dllexport linkage must be restored after optimizer failure"
    );
}

#[test]
fn test_dump_ir_contains_module_name() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "ir_dump_test");
    let ir = backend.dump_ir();
    assert!(
        ir.contains("ir_dump_test"),
        "dump_ir() should include the module name in the IR string"
    );
}

#[test]
fn modules_in_one_request_share_the_admitted_callable_buffer() {
    use molt_ir::backend_environment::{self, BackendEnvironment};
    use std::sync::Arc;
    let artifacts =
        cargo_test_artifacts::CargoTestArtifacts::new("llvm-callable-snapshot").unwrap();
    let path = artifacts.path().join("callables.txt");
    std::fs::write(&path, b"molt_init_sys\nmolt_main\n").unwrap();
    let payload = serde_json::json!({
        "MOLT_RUNTIME_CALLABLE_SYMBOLS": path.to_str().unwrap(),
        "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": "cd85e5b7627ffa5a6821fde44e31407d23a4d3da80a6e07a5b675af8d0925784",
    });
    let environment =
        Arc::new(BackendEnvironment::from_request(payload.as_object().unwrap()).unwrap());
    let _scope = backend_environment::enter(environment);
    let context = Context::create();
    let first = LlvmBackend::new(&context, "first");
    std::fs::remove_file(&path).unwrap();
    let second = LlvmBackend::new(&context, "second");
    assert!(Arc::ptr_eq(
        &first.runtime_callable_symbols,
        &second.runtime_callable_symbols
    ));
    assert_eq!(
        first
            .runtime_callable_symbols
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["molt_init_sys", "molt_main"]
    );
    // A fixture's intentional mutation still owns an independent set.
    let mut fixture = second;
    Arc::make_mut(&mut fixture.runtime_callable_symbols).remove("molt_main");
    assert!(first.runtime_callable_symbols.contains("molt_main"));
    assert!(!fixture.runtime_callable_symbols.contains("molt_main"));
}
