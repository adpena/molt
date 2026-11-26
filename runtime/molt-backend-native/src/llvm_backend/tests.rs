use super::*;
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::context::Context;
use inkwell::values::AnyValue;
use molt_ir::runtime_fn_generated::RuntimeFn;
use object::{Object, ObjectSection, ObjectSymbol};

#[test]
fn refcount_cfg_is_deterministic_and_inlined_at_every_optimization_level() {
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use molt_ir::deterministic_hash::HashMap;

    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-rc-determinism").unwrap();
    let mut first_ir = None;
    for level in [
        MoltOptLevel::O0,
        MoltOptLevel::O1,
        MoltOptLevel::O2,
        MoltOptLevel::O3,
    ] {
        let mut first_object = None;
        for attempt in 0..8 {
            let ctx = Context::create();
            let mut backend =
                LlvmBackend::new_for_target(&ctx, "rc_determinism", CodegenTarget::Host, level);
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
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![owner],
                results: vec![],
                attrs: AttrDict::default(),
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
            backend.optimize().unwrap();
            let path = artifacts
                .path()
                .join(format!("level-{level:?}-attempt-{attempt}.o"));
            backend.emit_object(&path).unwrap();
            let bytes = std::fs::read(path).unwrap();
            let object = object::File::parse(bytes.as_slice()).unwrap();
            assert!(
                !object.symbols().any(|symbol| symbol
                    .name()
                    .is_ok_and(|name| name.contains("__molt_inline_"))),
                "alwaysinline must remove RC wrapper symbols at level {level:?}"
            );
            let undefined = crate::test_support::native_object_symbols(&bytes).undefined;
            assert!(
                undefined.contains(RuntimeFn::IncRefObj.linker_symbol())
                    && undefined.contains(RuntimeFn::DecRefObj.linker_symbol()),
                "dynamic cold transitions must survive: {undefined:?}"
            );
            if let Some(expected) = &first_object {
                assert_eq!(
                    expected, &bytes,
                    "identical RC input changed object bytes at level {level:?}"
                );
            } else {
                first_object = Some(bytes);
            }
        }
    }
}

#[test]
fn refcount_sites_past_the_inline_budget_call_one_outlined_transition() {
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    let ctx = Context::create();
    let mut backend = LlvmBackend::new(&ctx, "rc_budget");
    let mut func = TirFunction::new(
        "rc_budget".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owner = func.blocks[&func.entry_block].args[0].id;
    let sites = 600;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for _ in 0..sites {
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![owner],
            results: vec![],
            attrs: AttrDict::default(),
            source_span: None,
        });
    }
    backend.function_repr_facts.insert(
        func.name.clone(),
        crate::representation_plan::ProducerCarrierPlan::build(&func),
    );
    super::lowering::lower_tir_to_llvm(&func, &backend);
    backend.module.verify().unwrap();
    let body = backend
        .module
        .get_compiled_function("rc_budget")
        .expect("lowered function")
        .print_to_string()
        .to_string();
    let calls = |helper: &str| body.matches(&format!("call void @{helper}")).count();
    let inline = calls(
        &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
            "__molt_inline_release",
        ),
    );
    let outlined = calls(
        &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
            "__molt_outlined_release",
        ),
    );
    assert_eq!(inline + outlined, sites);
    assert_eq!(
        outlined,
        sites - 512,
        "sites past the budget call the outlined copy"
    );
    backend.optimize().unwrap();
    backend.module.verify().unwrap();
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
    let backend = LlvmBackend::new_for_target(
        &ctx,
        "direct_object_abi",
        CodegenTarget::Host,
        MoltOptLevel::O0,
    );
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-object-abi")
        .expect("create LLVM object outputs within Cargo image custody");
    let artifact = artifacts.path().join(if cfg!(windows) {
        "backend.obj"
    } else {
        "backend.o"
    });
    backend
        .emit_object(&artifact)
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
fn wasm32_target_emits_a_relocatable_wasm_object_with_32_bit_pointers() {
    let ctx = Context::create();
    let backend = LlvmBackend::new_for_target(
        &ctx,
        "wasm32_object",
        CodegenTarget::Wasm32Wasip1,
        MoltOptLevel::O0,
    );
    // One position-independent object links statically or as a side module.
    assert_eq!(
        backend.target.reloc_mode(),
        inkwell::targets::RelocMode::PIC
    );
    assert_eq!(backend.pointer_sized_int_type().get_bit_width(), 32);
    let layout = backend.module.get_data_layout();
    let layout = layout.as_str().to_str().expect("UTF-8 data layout");
    assert!(layout.contains("p:32:32"), "wasm32 layout: {layout}");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-wasm32-object")
        .expect("create LLVM wasm32 object output");
    let artifact = artifacts.path().join("backend.o");
    backend
        .emit_object(&artifact)
        .expect("wasm32 object emission");
    let bytes = std::fs::read(&artifact).expect("read wasm32 object");
    assert_eq!(
        &bytes[..8],
        b"\0asm\x01\0\0\0",
        "a version-1 WebAssembly module"
    );
    // wasm-ld admits only relocatable objects, which carry a `linking` section.
    assert!(
        bytes
            .windows(b"linking".len())
            .any(|window| window == b"linking"),
        "a relocatable WebAssembly object"
    );
}

#[test]
#[cfg(feature = "native-backend")]
fn scoped_llvm_level_binds_the_effective_native_and_wasm_codegen_identity() {
    use molt_ir::backend_environment::{self, BackendEnvironment};
    for setting in [None, Some("O0"), Some("O1"), Some("O2"), Some("O3")] {
        let mut payload = serde_json::Map::new();
        if let Some(level) = setting {
            payload.insert("MOLT_LLVM_OPT_LEVEL".into(), serde_json::json!(level));
        }
        let _scope = backend_environment::enter(std::sync::Arc::new(
            BackendEnvironment::from_request(&payload).unwrap(),
        ));
        for target in [None, Some("wasm32-wasip1")] {
            let identity = crate::SimpleBackend::object_codegen_identity(target);
            assert_eq!(
                identity["llvm_target"]["optimization"],
                setting.unwrap_or("O3")
            );
        }
    }
    for invalid in ["", "O4", "speed"] {
        assert!(serde_json::from_value::<MoltOptLevel>(serde_json::json!(invalid)).is_err());
    }
}

#[test]
fn test_create_target_machine_all_levels() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "tm_test");
    assert_eq!(
        backend.target.reloc_mode(),
        inkwell::targets::RelocMode::PIC,
        "LLVM object emission must stay PIC-compatible for the shared runtime link path"
    );
    for (level, pipeline, machine) in [
        (MoltOptLevel::O0, "default<O0>", OptimizationLevel::None),
        (MoltOptLevel::O1, "default<O1>", OptimizationLevel::Less),
        (MoltOptLevel::O2, "default<O2>", OptimizationLevel::Default),
        (
            MoltOptLevel::O3,
            "default<O3>",
            OptimizationLevel::Aggressive,
        ),
    ] {
        assert_eq!(level.pipeline(), pipeline);
        assert_eq!(level.machine_level(), machine);
        let _machine = backend.target.create_target_machine(level);
    }
}

#[test]
fn test_optimize_empty_module_smoke() {
    let ctx = Context::create();
    let backend =
        LlvmBackend::new_for_target(&ctx, "opt_smoke", CodegenTarget::Host, MoltOptLevel::O2);
    // Running passes on an empty module must not panic or error.
    backend
        .optimize()
        .expect("empty-module optimization should succeed");
}

#[test]
fn test_optimize_invalid_pipeline_fails_closed_and_restores_linkage() {
    let ctx = Context::create();
    let backend = LlvmBackend::new_for_target(
        &ctx,
        "opt_fail_closed",
        CodegenTarget::Host,
        MoltOptLevel::O2,
    );
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
        .run_optimization_passes("not-a-real-pass")
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
fn modules_in_one_request_decode_one_admitted_callable_buffer() {
    use molt_ir::backend_environment::{self, BackendEnvironment};
    use std::sync::Arc;
    let artifacts =
        cargo_test_artifacts::CargoTestArtifacts::new("llvm-callable-snapshot").unwrap();
    let path = artifacts.path().join("callables.txt");
    std::fs::write(&path, b"molt_len\nmolt_main\n").unwrap();
    let payload = serde_json::json!({
        "MOLT_RUNTIME_CALLABLE_SYMBOLS": path.to_str().unwrap(),
        "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": "48fdbb5063ff53d73dcb52df55844e0069a21e917c09efbd1fc277400623bea5",
    });
    let environment =
        Arc::new(BackendEnvironment::from_request(payload.as_object().unwrap()).unwrap());
    let _scope = backend_environment::enter(environment);
    let context = Context::create();
    let first = LlvmBackend::new(&context, "first");
    std::fs::remove_file(&path).unwrap();
    let second = LlvmBackend::new(&context, "second");
    // The second module reads the request's admitted snapshot, not the file.
    assert_eq!(
        first.runtime_callable_symbols,
        second.runtime_callable_symbols
    );
    // The admission decodes to typed entries; `molt_main` is the program's
    // own entry, not a runtime entry, so it has no member.
    assert!(first.runtime_callable_symbols.contains(RuntimeFn::Len));
    assert!(!first.runtime_callable_symbols.contains(RuntimeFn::PrintObj));
    // A fixture's intentional mutation still owns an independent set.
    let mut fixture = second;
    fixture.runtime_callable_symbols.remove(RuntimeFn::Len);
    assert!(first.runtime_callable_symbols.contains(RuntimeFn::Len));
    assert!(!fixture.runtime_callable_symbols.contains(RuntimeFn::Len));
}
