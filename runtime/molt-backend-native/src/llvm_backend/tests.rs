use super::*;
use inkwell::context::Context;
use object::{Object, ObjectSection, ObjectSymbol};

#[test]
fn refcount_cfg_is_deterministic_and_inlined_at_every_optimization_level() {
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use std::collections::HashMap;

    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-rc-determinism").unwrap();
    let mut first_ir = None;
    for level in 0..3 {
        let opt = || match level {
            0 => MoltOptLevel::None,
            1 => MoltOptLevel::Speed,
            _ => MoltOptLevel::Aggressive,
        };
        let mut first_object = None;
        for attempt in 0..8 {
            let ctx = Context::create();
            let mut backend = LlvmBackend::new(&ctx, "rc_determinism");
            super::runtime_imports::declare_runtime_functions(&ctx, &backend.module);
            let mut func = TirFunction::new(
                "rc_determinism".into(),
                vec![TirType::Bool, TirType::DynBox],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let condition = func.blocks[&func.entry_block].args[0].id;
            let owner = func.blocks[&func.entry_block].args[1].id;
            let left = func.fresh_block();
            let right = func.fresh_block();
            let join = func.fresh_block();
            let rc = |opcode| TirOp {
                origin: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![owner],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            };
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(rc(OpCode::IncRef));
            entry.terminator = Terminator::CondBranch {
                cond: condition,
                then_block: left,
                then_args: vec![],
                else_block: right,
                else_args: vec![],
            };
            for (id, operations) in [
                (left, vec![rc(OpCode::IncRef), rc(OpCode::DecRef)]),
                (right, vec![rc(OpCode::DecRef), rc(OpCode::IncRef)]),
                (join, vec![rc(OpCode::DecRef)]),
            ] {
                func.blocks.insert(
                    id,
                    TirBlock {
                        id,
                        args: vec![],
                        ops: operations,
                        terminator: if id == join {
                            Terminator::Return { values: vec![] }
                        } else {
                            Terminator::Branch {
                                target: join,
                                args: vec![],
                            }
                        },
                    },
                );
            }
            // New hash seeds and reversed insertion order cannot change an
            // immutable constituent's bytes. No optimizer rewrites this graph.
            let mut rows: Vec<_> = func.blocks.drain().collect();
            rows.sort_by_key(|(id, _)| id.0);
            if attempt % 2 == 0 {
                rows.reverse();
            }
            func.blocks = rows.into_iter().collect::<HashMap<_, _>>();
            backend.function_repr_facts.insert(
                func.name.clone(),
                crate::representation_plan::ProducerCarrierPlan::build(&func),
            );
            super::lowering::lower_tir_to_llvm(&func, &backend);
            backend.module.verify().unwrap();
            let ir = backend.dump_ir();
            if let Some(expected) = &first_ir {
                assert_eq!(expected, &ir, "unordered TIR storage changed LLVM IR");
            } else {
                first_ir = Some(ir);
            }
            backend.optimize(opt()).unwrap();
            let path = artifacts
                .path()
                .join(format!("level-{level}-attempt-{attempt}.o"));
            backend.emit_object(&path, opt()).unwrap();
            let bytes = std::fs::read(path).unwrap();
            let object = object::File::parse(bytes.as_slice()).unwrap();
            assert!(
                !object.symbols().any(|symbol| symbol
                    .name()
                    .is_ok_and(|name| name.contains("__molt_inline_"))),
                "alwaysinline must remove RC wrapper symbols at level {level}"
            );
            let undefined = crate::test_support::native_object_symbols(&bytes).undefined;
            assert!(
                undefined.contains(&execution_abi::execution_symbol("molt_inc_ref_obj"))
                    && undefined.contains(&execution_abi::execution_symbol("molt_dec_ref_obj")),
                "dynamic cold transitions must survive: {undefined:?}"
            );
            if let Some(expected) = &first_object {
                assert_eq!(
                    expected, &bytes,
                    "identical RC input changed object bytes at level {level}"
                );
            } else {
                first_object = Some(bytes);
            }
        }
    }
}

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
