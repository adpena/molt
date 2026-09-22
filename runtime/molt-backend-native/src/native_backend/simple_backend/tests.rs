use super::backend_selection::{NativeCodegenBackend, select_native_codegen_backend};
use super::{
    DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT, DEFERRED_CODEGEN_FLUSH_OP_BUDGET,
    NativeBackendModuleContext, NativeRcAuthority, SimpleBackend, TrampolineKey,
    analyze_native_backend_ir, compute_function_has_ret, drain_cleanup_candidates,
    merge_closure_functions, merge_function_arities, merge_function_has_ret, merge_leaf_functions,
    merge_task_kinds, preprocess_backend_tir_input, should_flush_deferred_codegen,
};
use crate::ir::{FunctionIR, OpIR, SimpleIR};
use crate::{GENERATOR_CONTROL_BYTES, TrampolineKind};
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::condcodes::{CondCode, IntCC};
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{
    Block, BlockArg, ExternalName, Function, Inst, InstructionData, Opcode, Value, ValueDef,
};
use cranelift_module::{FuncId, Module};
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
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_main".to_string(),
            params: vec![],
            ops: if execution_context == crate::ir::ExecutionContextPolicy::Local {
                vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ]
            } else {
                vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }]
            },
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context,
        }],
        profile: None,
    };
    SimpleBackend::new().compile(ir).bytes
}

fn compile_function_to_clif(
    functions: Vec<FunctionIR>,
    target_name: &str,
) -> cranelift_codegen::ir::Function {
    compile_function_to_clif_with_imports(functions, target_name).function
}

struct CompiledFunctionClif {
    function: Function,
    import_ids: BTreeMap<&'static str, FuncId>,
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
        compiled.import_ids[crate::runtime_import_abi::MOLT_TASK_NEW.name],
    );
    assert_eq!(calls.len(), 1, "{}", function.display());
    let (block, call) = calls[0];
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

fn compile_function_to_clif_with_imports(
    functions: Vec<FunctionIR>,
    target_name: &str,
) -> CompiledFunctionClif {
    let backend = compile_selected_functions_direct(functions, &[target_name]);
    let function = backend
        .deferred_defines
        .iter()
        .find(|deferred| deferred.name == target_name)
        .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
        .func
        .clone();
    let import_ids = backend
        .import_ids
        .iter()
        .map(|(name, (id, _))| (*name, *id))
        .collect();
    CompiledFunctionClif {
        function,
        import_ids,
    }
}

/// Share direct function compilation between CLIF inspection and executable
/// tests without running a preprocessing pipeline that changes the RC authority.
pub(in crate::native_backend) fn compile_selected_functions_direct(
    functions: Vec<FunctionIR>,
    target_names: &[&str],
) -> SimpleBackend {
    let ir = SimpleIR {
        functions,
        profile: None,
    };
    let analysis = analyze_native_backend_ir(
        &ir,
        true,
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions),
    );
    let function_has_ret = compute_function_has_ret(&ir.functions);
    let function_arities = ir
        .functions
        .iter()
        .map(|func| (func.name.clone(), func.params.len()))
        .collect();
    let mut backend = SimpleBackend::new();
    for target_name in target_names {
        let target_func = ir
            .functions
            .iter()
            .find(|func| func.name == *target_name)
            .unwrap_or_else(|| panic!("missing target function `{target_name}`"))
            .clone();
        backend.compile_func(
            target_func,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
            &analysis.task_kinds,
            &analysis.task_closure_sizes,
            &analysis.defined_functions,
            &analysis.closure_functions,
            &analysis.leaf_functions,
            &function_arities,
            &function_has_ret,
        );
    }
    backend
}

/// Complete the same deferred codegen used by production without rerunning
/// preprocessing over the directly compiled test functions.
pub(in crate::native_backend) fn emit_direct_object(mut backend: SimpleBackend) -> Vec<u8> {
    backend.flush_deferred_defines();
    backend
        .module
        .finish()
        .emit()
        .expect("emit native test object")
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
    fn collect(
        function: &Function,
        cfg: &ControlFlowGraph,
        value: Value,
        visiting: &mut BTreeSet<Value>,
        sources: &mut BTreeSet<Value>,
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
                collect(function, cfg, left, visiting, sources);
                collect(function, cfg, right, visiting, sources);
            }
            ValueDef::Param(block, index) => {
                let mut found_incoming = false;
                for predecessor in cfg.pred_iter(block) {
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
                                collect(function, cfg, incoming, visiting, sources);
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
    collect(function, &cfg, value, &mut BTreeSet::new(), &mut sources);
    sources
}

fn value_originates_only_from(function: &Function, value: Value, source: Value) -> bool {
    let source = function.dfg.resolve_aliases(source);
    canonical_value_sources(function, value) == BTreeSet::from([source])
}

fn compile_function_to_clif_text(functions: Vec<FunctionIR>, target_name: &str) -> String {
    compile_function_to_clif(functions, target_name)
        .display()
        .to_string()
}

// Regression  native codegen must compile the CANONICAL bare `get_attr`.
//
// `tir::lower_to_simple` emits the canonical `get_attr` for any `LoadAttr`
// that carries no specialized `_original_kind` (its documented default,
// exactly like `set_attr`/`del_attr`/`index`/`call`). A TIR pass that
// yields a generic by-name attribute load reaches native codegen with that
// bare spelling  observed for `__future__._Feature.__repr__` under
// `--build-profile release`, where the guard-splitting passes leave a
// generic `get_attr` cold-fallback after specializing the
// `self.optional`/`.mandatory`/`.compiler_flag` `guarded_field_get`s. The
// attribute handler (`fc::attrs`) claimed every specialized `get_attr_*`
// alias but NOT the canonical `get_attr`, so the op hit the dispatch's loud
// no-codegen catch-all and panicked ("no codegen for result-producing op
// kind `get_attr`"). This compiles a function whose body is a bare
// `get_attr`; without the fix it panics in `compile_func`, with it the op
// routes to the generic-by-name attribute load.

fn roundtrip_function_through_tir(func: &FunctionIR) -> FunctionIR {
    let mut functions = vec![func.clone()];
    let cache_dir = test_tir_pipeline_cache_dir();
    let target_info = crate::tir::target_info::TargetInfo::native_release_fast();
    let run = crate::tir::pipeline_cache::run_cached_tir_pipeline(
        &mut functions,
        crate::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Native,
            cache_dir: Some(cache_dir.clone()),
            process_externs: false,
            verify_lir: true,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan_from_limits(
                1, None,
            ),
        },
        |function| preprocess_backend_tir_input(function, &target_info),
    );
    assert!(
        run.cached_tir.contains_function(&func.name),
        "shared TIR runner must return optimized TIR custody for '{}'",
        func.name
    );
    let _ = std::fs::remove_dir_all(cache_dir);
    let roundtripped = functions.pop().expect("roundtrip function missing");
    assert!(
        crate::tir::lower_to_simple::validate_labels(&roundtripped.ops),
        "TIR runner roundtrip must preserve all referenced labels: {:#?}",
        roundtripped.ops
    );
    roundtripped
}

fn test_tir_pipeline_cache_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "molt-tir-pipeline-cache-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

mod arith_codegen_snapshot;
mod backend_selection;
mod cleanup;
mod codegen_regressions;
mod compile_pipeline;
mod deferred_codegen;
mod fail_closed_codegen;
#[cfg(feature = "llvm")]
mod llvm_backend;
mod module_metadata;
mod tir_analysis;
mod trampolines;

mod field_access;
