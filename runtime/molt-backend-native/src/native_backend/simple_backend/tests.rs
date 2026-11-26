use super::backend_selection::{NativeCodegenBackend, select_native_codegen_backend};
use super::program_pipeline::preprocess_backend_tir_input;
use super::{
    DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT, DEFERRED_CODEGEN_FLUSH_OP_BUDGET,
    NativeBackendModuleContext, NativeFinalFunctions, SimpleBackend, should_flush_deferred_codegen,
};
use crate::GENERATOR_CONTROL_BYTES;
use crate::ir::{FunctionIR, OpIR, SimpleIR};
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::condcodes::{CondCode, IntCC};
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{
    Block, BlockArg, ExternalName, Function, Inst, InstructionData, Opcode, Value, ValueDef,
};
use cranelift_module::{FuncId, Module};
use molt_ir::OpKind;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

fn backend_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Acquire the process-env serialization lock, tolerating a poisoned mutex.
///
/// The guarded value is `()`  the lock exists only to *serialize* tests
/// that mutate process-global env vars (`MOLT_BACKEND`, `MOLT_STDLIB_OBJ`,
/// ) so they do not race. Each such test snapshots and restores the env
/// vars it touches itself; the mutex protects no shared in-memory invariant.
/// When one test panics while holding the guard, the mutex is *poisoned*,
/// but there is no corrupted state to guard against  the only thing the
/// poison flag would do is convert that single panic into a cascade of
/// `PoisonError` panics in every later test that takes the lock, hiding the
/// real failure behind noise. Recovering the guard via `into_inner()` keeps
/// the mutual-exclusion guarantee intact while letting the genuine failure
/// stand alone. This is the textbook-sound use of poison recovery: the
/// protected data carries no invariant the poison could have broken.
fn acquire_backend_env_lock() -> std::sync::MutexGuard<'static, ()> {
    backend_env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ScopedEnvVar {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    fn set(name: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var_os(name);
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        Self { name, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}

fn compile_trace_probe_object(
    emit_traces_env: Option<&str>,
    execution_context: crate::ir::ExecutionContextPolicy,
) -> Vec<u8> {
    let _guard = acquire_backend_env_lock();
    let _trace_env = ScopedEnvVar::set("MOLT_BACKEND_EMIT_TRACES", emit_traces_env);
    SimpleBackend::new()
        .compile(trace_probe_ir(execution_context))
        .bytes
}

fn trace_probe_ir(execution_context: crate::ir::ExecutionContextPolicy) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: if execution_context == crate::ir::ExecutionContextPolicy::Local {
                vec![
                    OpIR {
                        s_value: Some("molt_main".to_string()),
                        ..OpIR::new(OpKind::TraceEnterSlot)
                    },
                    // Observing locals makes the semantic record visible even
                    // when the compiler can elide records in pure leaves.
                    OpIR::new(OpKind::FrameLocals),
                    OpIR {
                        value: Some(1),
                        ..OpIR::new(OpKind::CheckException)
                    },
                    OpIR {
                        ..OpIR::new(OpKind::TraceExit)
                    },
                    OpIR {
                        ..OpIR::new(OpKind::RetVoid)
                    },
                    OpIR {
                        value: Some(1),
                        ..OpIR::new(OpKind::Label)
                    },
                    OpIR {
                        ..OpIR::new(OpKind::TraceExit)
                    },
                    OpIR {
                        ..OpIR::new(OpKind::RetVoid)
                    },
                ]
            } else {
                vec![OpIR {
                    ..OpIR::new(OpKind::RetVoid)
                }]
            },
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context,
        }],
        profile: None,
    }
}

/// A request from an object-code fixture whose runtime declares the full ABI.
/// JIT relocation tests can independently withhold a symbol from the actual
/// loader after codegen admits its signature.
fn enter_full_runtime_fixture() -> molt_ir::backend_environment::ScopeGuard {
    use molt_ir::backend_environment::{self, BackendEnvironment};
    use molt_ir::runtime_fn_generated::RuntimeFn;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, OnceLock};
    static ENVIRONMENT: OnceLock<Arc<BackendEnvironment>> = OnceLock::new();
    let environment = ENVIRONMENT.get_or_init(|| {
        let directory = test_tir_pipeline_cache_dir();
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("runtime-symbols");
        let symbols = RuntimeFn::ALL.iter().map(|f| f.symbol()).collect::<Vec<_>>().join("\n");
        std::fs::write(&path, &symbols).unwrap();
        let request = serde_json::json!({
            "MOLT_RUNTIME_CALLABLE_SYMBOLS": path,
            "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": molt_ir::content_digest::bytes_to_lower_hex(&Sha256::digest(symbols.as_bytes())),
        });
        let environment = Arc::new(BackendEnvironment::from_request(request.as_object().unwrap()).unwrap());
        let _scope = backend_environment::enter(Arc::clone(&environment));
        let admitted = molt_ir::runtime_callable_symbols::RuntimeCallableSymbols::capture();
        assert_eq!(admitted.symbols().unwrap().len(), RuntimeFn::ALL.len());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
        environment
    });
    backend_environment::enter(Arc::clone(environment))
}

fn compile_function_to_clif(
    functions: Vec<FunctionIR>,
    target_name: &str,
) -> cranelift_codegen::ir::Function {
    compile_function_to_clif_with_imports(functions, target_name).function
}

struct CompiledFunctionClif {
    function: Function,
    import_ids: BTreeMap<String, FuncId>,
}

fn attached_arguments(function: &Function, instruction: Inst) -> &[Value] {
    let arguments = function.dfg.inst_args(instruction);
    let context = function
        .dfg
        .block_params(function.layout.entry_block().unwrap())[0];
    assert_eq!(
        arguments[0], context,
        "attached call must forward the exact entry context"
    );
    &arguments[1..]
}

fn declared_import_ids(module: &dyn Module) -> BTreeMap<String, FuncId> {
    // Direct runtime calls and helper calls share the module's declarations;
    // the helper cache contains only calls made through import_runtime_func_ref.
    // The program's terminal-release helper is local, and every release
    // site's slow call reaches it.
    module
        .declarations()
        .get_functions()
        .filter_map(|(id, declaration)| declaration.name.as_ref().map(|name| (
            name.strip_suffix(molt_ir::runtime_callable_abi_generated::ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX)
                .unwrap_or(name).to_owned(), id, declaration.linkage)))
        .filter(|(name, _, linkage)| {
            *linkage == cranelift_module::Linkage::Import
                || name == crate::codegen_ssa::RELEASE_TERMINAL_HELPER
        })
        .map(|(name, id, _)| (name, id))
        .collect()
}

/// Every call that releases an owner: `molt_dec_ref_obj`, and the program's
/// terminal-release helper, which each release site's slow call reaches (it
/// frees an unobserved plain instance and calls `molt_dec_ref_obj` otherwise).
fn release_call_sites(function: &Function, ids: &BTreeMap<String, FuncId>) -> Vec<(Block, Inst)> {
    let mut sites: Vec<_> = [
        molt_ir::runtime_fn_generated::RuntimeFn::DecRefObj.symbol(),
        crate::codegen_ssa::RELEASE_TERMINAL_HELPER,
    ]
    .into_iter()
    .filter_map(|name| ids.get(name))
    .flat_map(|&id| call_sites_for_import(function, id))
    .collect();
    sites.sort();
    sites
}

fn definition(func: &Function, value: Value) -> Option<Inst> {
    match func.dfg.value_def(func.dfg.resolve_aliases(value)) {
        ValueDef::Result(inst, _) => Some(inst),
        ValueDef::Param(_, _) | ValueDef::Union(_, _) => None,
    }
}

fn constant(func: &Function, value: Value) -> Option<i64> {
    match func.dfg.insts[definition(func, value)?] {
        InstructionData::UnaryImm {
            opcode: Opcode::Iconst,
            imm,
        } => Some(imm.bits()),
        _ => None,
    }
}

/// Find the nonconstant operand of `operand code immediate`, independent of
/// value numbering, aliases, printed CLIF spelling, and comparison orientation.
fn masked_operand(func: &Function, value: Value, mask: i64) -> Option<Value> {
    let inst = definition(func, value)?;
    if func.dfg.insts[inst].opcode() != Opcode::Band {
        return None;
    }
    let [left, right] = func.dfg.inst_args(inst) else {
        return None;
    };
    if constant(func, *left) == Some(mask) {
        Some(*right)
    } else if constant(func, *right) == Some(mask) {
        Some(*left)
    } else {
        None
    }
}

fn comparison_operand(
    func: &Function,
    condition: Value,
    code: IntCC,
    immediate: i64,
) -> Option<Value> {
    let inst = definition(func, condition)?;
    let actual_code = func.dfg.insts[inst].cond_code()?;
    let [left, right] = func.dfg.inst_args(inst) else {
        return None;
    };
    if actual_code == code && constant(func, *right) == Some(immediate) {
        Some(*left)
    } else if actual_code == code.swap_args() && constant(func, *left) == Some(immediate) {
        Some(*right)
    } else {
        None
    }
}

/// Both ordinary task producers and callable trampolines must branch on the
/// allocator's exact result before any payload or completion side effects.
/// Return the allocation value and semantic success/failure destinations.
fn assert_task_allocation_admission(compiled: &CompiledFunctionClif) -> (Value, Block, Block) {
    let function = &compiled.function;
    let calls = call_sites_for_import(
        function,
        compiled.import_ids[molt_ir::runtime_fn_generated::RuntimeFn::TaskNew.symbol()],
    );
    assert_eq!(calls.len(), 1, "{}", function.display());
    assert_task_allocation_admission_for(compiled, calls[0])
}

fn assert_task_allocation_admission_for(
    compiled: &CompiledFunctionClif,
    (block, call): (Block, Inst),
) -> (Value, Block, Block) {
    let function = &compiled.function;
    let task = function.dfg.first_result(call);
    let branch = function
        .layout
        .last_inst(block)
        .expect("allocation block terminator");
    let InstructionData::Brif { blocks, .. } = &function.dfg.insts[branch] else {
        panic!(
            "allocation must branch before initialization:\n{}",
            function.display()
        );
    };
    let condition = function.dfg.inst_args(branch)[0];
    let code = [IntCC::Equal, IntCC::NotEqual]
        .into_iter()
        .find(|&code| {
            comparison_operand(function, condition, code, molt_codegen_abi::box_none_bits())
                .is_some_and(|operand| value_originates_only_from(function, operand, task))
        })
        .unwrap_or_else(|| {
            panic!(
                "guard must compare task_new's exact result with boxed None:\n{}",
                function.display()
            )
        });
    for inst in function
        .layout
        .block_insts(block)
        .skip_while(|&inst| inst != call)
        .skip(1)
    {
        let opcode = function.dfg.insts[inst].opcode();
        assert!(
            !opcode.can_load() && !opcode.can_store() && !opcode.is_call(),
            "allocation must branch before payload access, RC, registration, or wrapping:\n{}",
            function.display()
        );
    }
    let success_index = usize::from(code == IntCC::Equal);
    let success = blocks[success_index].block(&function.dfg.value_lists);
    let failure = blocks[1 - success_index].block(&function.dfg.value_lists);
    assert_ne!(success, failure, "{}", function.display());
    let cfg = ControlFlowGraph::with_function(function);
    let predecessors: Vec<_> = cfg.pred_iter(success).collect();
    assert_eq!(predecessors.len(), 1, "{}", function.display());
    assert_eq!(predecessors[0].inst, branch, "{}", function.display());
    (task, success, failure)
}

/// Compiled accesses to an object's reference-count word. A generated plain
/// transition reads and writes only this header word; every other mode
/// reaches the runtime transaction without touching it.
fn refcount_word_accesses(function: &Function) -> Vec<Inst> {
    function
        .layout
        .blocks()
        .flat_map(|block| function.layout.block_insts(block))
        .filter(|&inst| {
            let data = &function.dfg.insts[inst];
            (data.opcode().can_load() || data.opcode().can_store())
                && data.load_store_offset() == Some(molt_codegen_abi::HEADER_REFCOUNT_OFFSET)
        })
        .collect()
}

/// The refcount admission is the thread-record word
/// `THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET`, loaded once at entry
/// (`docs/design/foundation/inline_refcount.md`). A body uses exactly one
/// admission load when it touches a count word, and none when every
/// transition reaches the runtime: debug and atomic compilers, or
/// immediate-only bodies leave the load dead. Each count-word access must lie
/// under the arm that accepts either grant carrying plain storage. The oracle is
/// the emitted body, so it holds whichever storage mode built the compiler.
/// Returns every admission load and the record load that addresses it.
fn assert_refcount_word_access_is_admitted(compiled: &CompiledFunctionClif) -> Vec<(Block, Inst)> {
    use molt_codegen_abi::inline_refcount::CompiledRefcountAdmission::{
        AttachedFrameOwners, AttachedPlain,
    };

    let function = &compiled.function;
    let used = |inst: Inst| {
        let result = function.dfg.first_result(inst);
        function.layout.blocks().any(|block| {
            function
                .layout
                .block_insts(block)
                .any(|user| function.dfg.inst_args(user).contains(&result))
        })
    };
    let admissions: Vec<(Block, Inst)> = function
        .layout
        .blocks()
        .flat_map(|block| {
            function
                .layout
                .block_insts(block)
                .map(move |inst| (block, inst))
        })
        .filter(|&(_, inst)| {
            matches!(
                function.dfg.insts[inst],
                InstructionData::Load { offset, .. }
                    if i32::from(offset)
                        == molt_codegen_abi::THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET
            )
        })
        .collect();
    // The admission's address is the entry's thread-record load.
    let admissions: Vec<(Block, Inst)> = admissions
        .into_iter()
        .flat_map(|(block, load)| {
            let address = function.dfg.inst_args(load)[0];
            let record = function
                .dfg
                .value_def(address)
                .inst()
                .filter(|&def| matches!(function.dfg.insts[def], InstructionData::Load { .. }));
            std::iter::once((block, load)).chain(record.map(|def| {
                (
                    function.layout.inst_block(def).expect("placed record load"),
                    def,
                )
            }))
        })
        .collect();
    let accesses = refcount_word_accesses(function);
    assert_eq!(
        admissions.iter().filter(|&&(_, inst)| matches!(
            function.dfg.insts[inst],
            InstructionData::Load { offset, .. }
                if i32::from(offset) == molt_codegen_abi::THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET
        ) && used(inst)).count(),
        usize::from(!accesses.is_empty()),
        "one used invocation admission exactly when compiled code touches a count word:\n{}",
        function.display()
    );
    let Some(&(_, admission_load)) = admissions.iter().find(|&&(_, inst)| {
        used(inst)
            && matches!(
                function.dfg.insts[inst],
                InstructionData::Load { offset, .. }
                    if i32::from(offset)
                        == molt_codegen_abi::THREAD_RECORD_REFCOUNT_ADMISSION_OFFSET
            )
    }) else {
        return admissions;
    };
    let admission = function.dfg.first_result(admission_load);
    let cfg = ControlFlowGraph::with_function(function);
    let dominators =
        cranelift_codegen::dominator_tree::DominatorTree::with_function(function, &cfg);
    let admitted_arms: Vec<Block> = function
        .layout
        .blocks()
        .filter_map(|block| {
            let branch = function.layout.last_inst(block)?;
            let InstructionData::Brif { blocks, .. } = &function.dfg.insts[branch] else {
                return None;
            };
            let condition = function.dfg.inst_args(branch)[0];
            let granted =
                comparison_operand(function, condition, IntCC::Equal, AttachedPlain as i64)?;
            let plain_mask = !(AttachedFrameOwners as i64 ^ AttachedPlain as i64);
            let loaded = masked_operand(function, granted, plain_mask)?;
            if !value_originates_only_from(function, loaded, admission) {
                return None;
            }
            let admitted = blocks[0].block(&function.dfg.value_lists);
            let predecessors: Vec<_> = cfg.pred_iter(admitted).collect();
            (predecessors.len() == 1 && predecessors[0].inst == branch).then_some(admitted)
        })
        .collect();
    for access in accesses {
        assert!(
            admitted_arms
                .iter()
                .any(|&arm| dominators.dominates(arm, access, &function.layout)),
            "count-word access {access} bypasses the plain-storage admission check:\n{}",
            function.display()
        );
    }
    admissions
}

fn compile_function_to_clif_with_imports(
    functions: Vec<FunctionIR>,
    target_name: &str,
) -> CompiledFunctionClif {
    compiled_clif(
        compile_selected_functions_direct(functions, &[target_name]),
        target_name,
    )
}

fn compiled_clif(backend: SimpleBackend, target_name: &str) -> CompiledFunctionClif {
    let function = backend
        .deferred_defines
        .iter()
        .find(|deferred| deferred.name == target_name)
        .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
        .func
        .clone();
    let import_ids = declared_import_ids(&backend.module);
    CompiledFunctionClif {
        function,
        import_ids,
    }
}

/// Fixtures for the final typed boundary exercise its real source projection.
/// Direct codegen controls below instead retain their authored final body.
fn final_functions_for_test(functions: &[FunctionIR]) -> NativeFinalFunctions {
    NativeFinalFunctions::from_codegen_fixtures(SimpleIR {
        functions: functions.to_vec(),
        profile: None,
    })
}

#[test]
fn direct_codegen_fixture_custody_preserves_authored_body_and_required_facts() {
    let pure = FunctionIR {
        name: "direct_fixture_pure".into(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        params: vec!["borrowed".into()],
        param_types: Some(vec!["dyn".into()]),
        source_file: Some("fixture.py".into()),
        ops: vec![
            OpIR {
                ..OpIR::new(OpKind::DropInserted)
            },
            OpIR {
                out: Some("limb".into()),
                value: Some(1_i64 << 31),
                ..OpIR::new(OpKind::ConstInt)
            },
            OpIR {
                args: Some(vec!["limb".into(), "limb".into()]),
                var: Some("wide".into()),
                out: Some("overflow".into()),
                source_line: Some(17),
                source_op_idx: Some(91),
                ..OpIR::new(OpKind::CheckedMul)
            },
            OpIR {
                args: Some(vec!["wide".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..Default::default()
    };
    let mut unfinalized = pure.clone();
    unfinalized.name = "direct_fixture_unfinalized".into();
    unfinalized.ops.remove(0);
    let mut callback = pure.clone();
    callback.name = "direct_fixture_callback".into();
    callback.ops.insert(
        1,
        OpIR {
            args: Some(vec!["borrowed".into()]),
            ..OpIR::new(OpKind::CallFunc)
        },
    );
    // The preserved tuple constructor has conservative callback effects: an
    // independent non-leaf control.
    let mut allocation = pure.clone();
    allocation.name = "direct_fixture_allocation".into();
    allocation.ops.insert(
        allocation.ops.len() - 1,
        OpIR {
            args: Some(vec!["borrowed".into()]),
            out: Some("pair".into()),
            ..OpIR::new(OpKind::TupleNew)
        },
    );
    allocation.ops.last_mut().unwrap().args = Some(vec!["pair".into()]);
    let ir = SimpleIR {
        functions: vec![pure, unfinalized, callback, allocation],
        profile: None,
    };
    // The declaration and every authored TIR body must survive separately.
    let bodies: Vec<_> = ir
        .functions
        .iter()
        .map(|source| {
            crate::tir::printer::print_function(&crate::tir::lower_from_simple::lower_to_tir(
                source,
            ))
        })
        .collect();
    let mut headers = ir.clone();
    for source in &mut headers.functions {
        source.ops.clear();
    }
    let fixtures = NativeFinalFunctions::from_codegen_fixtures(ir);
    assert_eq!(
        serde_json::to_vec(fixtures.ir()).unwrap(),
        serde_json::to_vec(&headers).unwrap(),
        "only declaration metadata survives the source handoff"
    );
    assert_eq!(
        tir_analysis::terminal_leaf_oracle(&fixtures),
        BTreeSet::from(["direct_fixture_pure".into()]),
        "eligibility requires the actual full-drop marker and absence of callbacks"
    );
    for (id, body) in fixtures.into_local_functions() {
        assert_eq!(
            crate::tir::printer::print_function(&body.tir),
            bodies[id.index()]
        );
        let checked = body
            .tir
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find(|op| op.opcode == crate::tir::ops::OpCode::CheckedMul)
            .unwrap();
        let &[product, overflow] = checked.results.as_slice() else {
            panic!("checked multiplication produces a product and an overflow bit");
        };
        assert!(body.carriers.is_full_deopt_int(product));
        assert!(!body.carriers.is_full_deopt_int(overflow));
        assert!(
            !body
                .carriers
                .is_full_deopt_int(body.tir.blocks[&body.tir.entry_block].args[0].id)
        );
    }
}

/// Independent former whole-module oracle, retained only in tests. It observes
/// actual final SimpleIR through the real shared call graph and drop authority.
fn final_leaf_oracle(functions: &[FunctionIR]) -> BTreeSet<String> {
    let table = molt_ir::FunctionTable::of(functions);
    let bodies = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| !function.is_extern)
        .map(|(index, function)| {
            (
                molt_ir::FunctionId::from_index(index),
                crate::tir::lower_from_simple::lower_to_tir(function),
            )
        });
    graph_leaf_oracle(table, bodies)
}

fn graph_leaf_oracle(
    table: molt_ir::FunctionTable,
    bodies: impl Iterator<Item = (molt_ir::FunctionId, crate::tir::TirFunction)>,
) -> BTreeSet<String> {
    let bodies = bodies
        .filter(|(_, function)| {
            matches!(
                function
                    .attrs
                    .get(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
                Some(crate::tir::ops::AttrValue::Bool(true)),
            )
        })
        .collect();
    let module =
        crate::tir::TirModule::from_document("native_leaf_test_oracle", table.clone(), bodies);
    crate::tir::CallGraph::build(&module)
        .leaf_functions()
        .iter()
        .map(|id| table.symbol(id).to_string())
        .collect()
}

/// Share direct function compilation between CLIF inspection and executable
/// tests without running a preprocessing pipeline that changes the RC authority.
pub(in crate::native_backend) fn compile_selected_functions_direct(
    functions: Vec<FunctionIR>,
    target_names: &[&str],
) -> SimpleBackend {
    compile_selected_with_context(functions, target_names, None)
}

fn compile_selected_with_context(
    functions: Vec<FunctionIR>,
    target_names: &[&str],
    context: Option<NativeBackendModuleContext>,
) -> SimpleBackend {
    compile_final_functions(
        NativeFinalFunctions::from_codegen_fixtures(SimpleIR {
            functions,
            profile: None,
        }),
        target_names,
        context,
    )
}

/// Compile `target_names` of final functions as production does, function by
/// function, without its preprocessing.
fn compile_final_functions(
    finalized: NativeFinalFunctions,
    target_names: &[&str],
    context: Option<NativeBackendModuleContext>,
) -> SimpleBackend {
    let ir = finalized.ir();
    let mut backend = SimpleBackend::new();
    if let Some(context) = context {
        context
            .validate_function_linkage_abis(&ir.functions)
            .expect("provider and consumer agree");
        backend.set_module_context(context);
    }
    let table = &molt_ir::FunctionTable::of(&ir.functions);
    let (declarations, linkage) = backend.declare_final_functions(&finalized, table);
    // Object fixtures have the full import ABI. Runtime-profile admission
    // is exercised separately with the selected runtime's manifest.
    let runtime = crate::codegen_ssa::differential::every_runtime_entry();
    let mut remaining: BTreeSet<_> = target_names.iter().copied().collect();
    for (id, function) in finalized.into_local_functions() {
        if !remaining.remove(function.tir.name.as_str()) {
            continue;
        }
        backend
            .build_walker_definition(
                declarations[id.index()],
                &function,
                table,
                &linkage,
                &runtime,
            )
            .unwrap_or_else(|errors| panic!("fixture lowering failed:\n{}", errors.join("\n")));
    }
    assert!(
        remaining.is_empty(),
        "missing direct fixture functions: {remaining:?}"
    );
    backend
}

/// Freeze the provider in the complete document, then compile the caller as a separate object.
fn compile_with_provider(backend: &mut SimpleBackend, caller: FunctionIR, provider: FunctionIR) {
    let mut functions = vec![caller, provider];
    let context = SimpleBackend::prepare_module_context(&mut functions);
    backend.set_module_context(context);
    crate::externalize_function_with_signature(&mut functions[1]);
    let finalized = NativeFinalFunctions::from_codegen_fixtures(SimpleIR {
        functions,
        profile: None,
    });
    let table = molt_ir::FunctionTable::of(&finalized.ir().functions);
    let (declarations, linkage) = backend.declare_final_functions(&finalized, &table);
    // Object fixtures have the full import ABI. Runtime-profile admission
    // is exercised separately with the selected runtime's manifest.
    let runtime = crate::codegen_ssa::differential::every_runtime_entry();
    for (id, definition) in finalized.into_local_functions() {
        backend
            .build_walker_definition(
                declarations[id.index()],
                &definition,
                &table,
                &linkage,
                &runtime,
            )
            .unwrap_or_else(|errors| panic!("fixture lowering failed: {}", errors.join("\n")));
    }
}

/// Complete the same deferred codegen used by production without rerunning
/// preprocessing over the directly compiled test functions.
pub(in crate::native_backend) fn emit_direct_object(mut backend: SimpleBackend) -> Vec<u8> {
    backend.flush_deferred_defines(None);
    backend
        .module
        .into_object()
        .finish()
        .emit()
        .expect("emit native test object")
}

#[cfg(feature = "llvm")]
impl SimpleBackend {
    /// Emit the same typed bodies and provider declarations as a dual-backend
    /// executable fixture through Cranelift's production definition path.
    pub(crate) fn emit_typed_fixture(
        bodies: Vec<crate::tir::TirFunction>,
        table: &molt_ir::FunctionTable,
        linkage: &BTreeMap<String, molt_ir::linkage_abi::NativeFunctionLinkageAbi>,
    ) -> Vec<u8> {
        let mut backend = Self::new();
        let runtime = crate::codegen_ssa::differential::every_runtime_entry();
        for tir in bodies {
            let signature = crate::native_backend::codegen_builder::linkage_signature(
                &backend.module,
                &linkage[&tir.name],
            );
            let id = crate::native_backend::declare_compiled_execution_function(
                &mut backend.module,
                &tir.name,
                cranelift_module::Linkage::Export,
                &signature,
            )
            .unwrap();
            let carriers = crate::representation_plan::ProducerCarrierPlan::build(&tir);
            backend
                .build_walker_definition(
                    id,
                    &super::NativeFinalFunction { tir, carriers },
                    table,
                    linkage,
                    &runtime,
                )
                .unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
        }
        emit_direct_object(backend)
    }
}

fn call_sites_for_import(function: &Function, import_id: FuncId) -> Vec<(Block, Inst)> {
    function
        .layout
        .blocks()
        .flat_map(|block| {
            function
                .layout
                .block_insts(block)
                .map(move |inst| (block, inst))
        })
        .filter(|(_, inst)| {
            let InstructionData::Call { func_ref, .. } = function.dfg.insts[*inst] else {
                return false;
            };
            let ExternalName::User(name) = function.dfg.ext_funcs[func_ref].name else {
                return false;
            };
            let name = &function.params.user_named_funcs()[name];
            name.namespace == 0 && name.index == import_id.as_u32()
        })
        .collect()
}

/// Resolve an observed SSA value to its instruction/function-parameter
/// origins through Cranelift aliases and explicit block transport.
fn canonical_value_sources(function: &Function, value: Value) -> BTreeSet<Value> {
    canonical_value_sources_from(function, value, None)
}

/// Restrict phi predecessors to the observed continuation when proving a
/// successful arm: unrelated error exits may join the same final return.
fn canonical_value_sources_from(
    function: &Function,
    value: Value,
    reachable: Option<&BTreeSet<Block>>,
) -> BTreeSet<Value> {
    fn collect(
        function: &Function,
        cfg: &ControlFlowGraph,
        value: Value,
        visiting: &mut BTreeSet<Value>,
        sources: &mut BTreeSet<Value>,
        reachable: Option<&BTreeSet<Block>>,
    ) {
        let value = function.dfg.resolve_aliases(value);
        if !visiting.insert(value) {
            sources.insert(value);
            return;
        }
        match function.dfg.value_def(value) {
            ValueDef::Result(_, _) => {
                sources.insert(value);
            }
            ValueDef::Union(left, right) => {
                collect(function, cfg, left, visiting, sources, reachable);
                collect(function, cfg, right, visiting, sources, reachable);
            }
            ValueDef::Param(block, index) => {
                let mut found_incoming = false;
                for predecessor in cfg.pred_iter(block) {
                    if reachable.is_some_and(|reachable| {
                        reachable.contains(&block) && !reachable.contains(&predecessor.block)
                    }) {
                        continue;
                    }
                    let destinations = function.dfg.insts[predecessor.inst].branch_destination(
                        &function.dfg.jump_tables,
                        &function.dfg.exception_tables,
                    );
                    for destination in destinations
                        .iter()
                        .filter(|destination| destination.block(&function.dfg.value_lists) == block)
                    {
                        let Some(argument) = destination.args(&function.dfg.value_lists).nth(index)
                        else {
                            continue;
                        };
                        found_incoming = true;
                        match argument {
                            BlockArg::Value(incoming) => {
                                collect(function, cfg, incoming, visiting, sources, reachable);
                            }
                            BlockArg::TryCallRet(_) | BlockArg::TryCallExn(_) => {
                                sources.insert(value);
                            }
                        }
                    }
                }
                if !found_incoming {
                    // Root block parameters (including function parameters)
                    // are canonical because they have no incoming CFG value.
                    sources.insert(value);
                }
            }
        }
        visiting.remove(&value);
    }

    let cfg = ControlFlowGraph::with_function(function);
    let mut sources = BTreeSet::new();
    collect(
        function,
        &cfg,
        value,
        &mut BTreeSet::new(),
        &mut sources,
        reachable,
    );
    sources
}

fn value_originates_only_from(function: &Function, value: Value, source: Value) -> bool {
    let source = function.dfg.resolve_aliases(source);
    canonical_value_sources(function, value) == BTreeSet::from([source])
}

/// The (failure, success) targets of the exception check that ends `block`.
/// A check that holds the thread record tests its pending byte inline and
/// branches to a confirm block that calls the runtime reader and jumps to a
/// join; the join's branch is the semantic exception edge (invariant J1).
fn exception_guard_edges(
    function: &Function,
    block: cranelift_codegen::ir::Block,
) -> (cranelift_codegen::ir::Block, cranelift_codegen::ir::Block) {
    let targets = |block: cranelift_codegen::ir::Block| {
        let last = function.layout.last_inst(block)?;
        match &function.dfg.insts[last] {
            cranelift_codegen::ir::InstructionData::Brif { blocks, .. } => Some((
                blocks[0].block(&function.dfg.value_lists),
                blocks[1].block(&function.dfg.value_lists),
            )),
            _ => None,
        }
    };
    let (taken, not_taken) = targets(block).unwrap_or_else(|| {
        panic!(
            "{block} must end in an exception branch:\n{}",
            function.display()
        )
    });
    for (confirm, join) in [(taken, not_taken), (not_taken, taken)] {
        if function.layout.last_inst(confirm).is_some_and(|last| {
            matches!(&function.dfg.insts[last], InstructionData::Jump { destination, .. }
                if destination.block(&function.dfg.value_lists) == join)
        }) {
            return targets(join).expect("the pending confirmation must branch");
        }
    }
    (taken, not_taken)
}

fn compile_function_to_clif_text(functions: Vec<FunctionIR>, target_name: &str) -> String {
    compile_function_to_clif(functions, target_name)
        .display()
        .to_string()
}

/// Run the production module optimizer and terminal lifetime finalization on
/// the same document the machine test compiles. Multiple bodies share one id
/// space, including calls between the fixture's functions.
fn finalize_through_tir(mut functions: Vec<FunctionIR>) -> NativeFinalFunctions {
    molt_ir::function_table::resolve_function_references(&mut functions).unwrap();
    let cache_dir = test_tir_pipeline_cache_dir();
    let target_info = crate::tir::target_info::TargetInfo::native_release_fast();
    let mut cached = crate::tir::pipeline_cache::run_cached_tir_pipeline(
        &functions,
        crate::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Native,
            cache: crate::tir::pipeline_cache::TirCacheLocation::Directory(cache_dir.clone()),
            process_externs: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan_from_limits(
                1, None,
            ),
        },
        preprocess_backend_tir_input,
    )
    .cached_tir;
    let _ = std::fs::remove_dir_all(cache_dir);
    for function in functions.iter().filter(|function| !function.is_extern) {
        assert!(
            cached.contains_function(&function.name),
            "missing optimized body {}",
            function.name
        );
    }
    // Each named body is an independently observable fixture entry.
    let non_inlinable = functions
        .iter()
        .map(|function| function.name.clone())
        .collect();
    let typed = crate::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
        &functions,
        &mut cached,
        crate::tir::pipeline_cache::TirOwnedModulePipelineOptions {
            cell_registry: None,
            target_info: &target_info,
            module_name: "native_module",
            non_inlinable: &non_inlinable,
            missing_tir_context: "native final function custody",
            mode: crate::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase,
            linkage: Some(&molt_ir::linkage_abi::document_signatures(&functions)),
        },
    );
    typed.append_clone_rows(&mut functions);
    let finalized = NativeFinalFunctions::from_final_tir(
        SimpleIR {
            functions,
            profile: None,
        },
        typed.functions,
    );
    for body in finalized.bodies() {
        crate::tir::verify::verify_function(body).expect("valid final TIR control flow and SSA");
    }
    finalized
}

/// Inspect instructions and exact machine imports after drop insertion.
fn compile_terminal_fixture(functions: Vec<FunctionIR>, target: &str) -> CompiledFunctionClif {
    compiled_clif(
        compile_final_functions(finalize_through_tir(functions), &[target], None),
        target,
    )
}

/// The CLIF of `target_name`, compiled from final functions.
fn compile_final_to_clif_text(source: &FunctionIR) -> String {
    let target_name = source.name.as_str();
    compile_final_functions(
        finalize_through_tir(vec![source.clone()]),
        &[target_name],
        None,
    )
    .deferred_defines
    .iter()
    .find(|deferred| deferred.name == target_name)
    .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
    .func
    .display()
    .to_string()
}

pub(super) fn test_tir_pipeline_cache_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "molt-tir-pipeline-cache-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

mod activation_exits;
mod arith_codegen_snapshot;
mod backend_selection;
mod binding_and_text;
mod call_family_objects;
mod call_operand_custody;
mod codegen_regressions;
mod compile_pipeline;
mod deferred_codegen;
mod fail_closed_codegen;
#[cfg(feature = "llvm")]
mod llvm_backend;
mod module_metadata;
mod native_callable_dispatch;
mod tir_analysis;
mod trampolines;

mod field_access;

fn physical_runtime_symbol(logical: &str) -> &'static str {
    molt_ir::runtime_callable_abi_generated::runtime_execution_abi(logical)
        .unwrap_or_else(|| panic!("missing runtime execution ABI: {logical}"))
        .physical_symbol
}

mod attributes;
