use super::*;

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub fn compile(mut self, ir: SimpleIR) -> CompileOutput {
        let timing = env_setting("MOLT_BACKEND_TIMING")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        // Backend selection: MOLT_BACKEND=llvm is an explicit contract. A
        // missing LLVM feature must fail closed instead of substituting a
        // different backend and producing misleading validation evidence.
        let backend_setting = env_setting("MOLT_BACKEND");
        let use_llvm = backend_setting_requests_llvm(backend_setting.as_deref());
        assert_requested_llvm_backend_available(use_llvm);
        let prepared = self.prepare_program_for_codegen(&mut ir, use_llvm, timing, &compile_start);
        let emit_resolver_here = prepared.emit_resolver_here;
        let app_intrinsic_manifest = prepared.app_intrinsic_manifest;
        let pre_split_task_kinds = prepared.pre_split_task_kinds;
        let pre_split_task_closure_sizes = prepared.pre_split_task_closure_sizes;
        //  LLVM backend dispatch
        // When MOLT_BACKEND=llvm and the llvm feature is compiled in, route
        // through the LLVM backend instead of Cranelift.  Each function is
        // lifted to TIR, lowered to LLVM IR, then the whole module is
        // optimized and emitted as a native object file.
        #[cfg(feature = "llvm")]
        if use_llvm {
            use crate::llvm_backend::{LlvmBackend, MoltOptLevel};

            let context = inkwell::context::Context::create();
            let mut llvm = LlvmBackend::new(&context, "molt_module");

            // Declare all runtime functions that lowered code may call into.
            crate::llvm_backend::runtime_imports::declare_runtime_functions(
                llvm.context,
                &llvm.module,
            );

            let func_count = ir.functions.iter().filter(|f| !f.is_extern).count();
            let total_ops: usize = ir
                .functions
                .iter()
                .filter(|f| !f.is_extern)
                .map(|f| f.ops.len())
                .sum();
            eprintln!(
                "MOLT_BACKEND(llvm): compiling {func_count} functions ({total_ops} total ops)"
            );
            let codegen_start = std::time::Instant::now();

            // Unified cost model (Tier-0 S2) for the LLVM target, derived from
            // the host CPU's vector feature string so the vectorizer can size
            // lanes to the actual machine (behavior-neutral today: the width is
            // a dead annotation, but structurally correct for real SIMD codegen).
            let llvm_tti = crate::tir::target_info::TargetInfo::from_llvm_feature_string(
                inkwell::targets::TargetMachine::get_host_cpu_features()
                    .to_str()
                    .unwrap_or(""),
            );
            // Fuse `obj.method(args)` / `super().method(args)` dispatch into the
            // allocation-free `call_method_ic` / `call_super_method_ic` ops
            // BEFORE lifting to TIR. Unlike the Cranelift path (which fuses the
            // post-roundtrip SimpleIR immediately before `compile_func`), the
            // LLVM path lowers directly from the per-function-optimized TIR, so
            // the IC ops enter the TIR roundtrip as first-class TIR opcodes
            // and lower through dedicated LLVM opcode arms. Built from the same
            // fused `func` as `function_repr_facts`, keeping the SimpleIR /
            // TIR pair aligned. Extern (declaration-only) functions have empty
            // bodies  nothing to fuse.
            for func in &mut ir.functions {
                if !func.is_extern {
                    fuse_method_dispatch(func);
                }
            }
            let mut llvm_cached_tir = crate::tir::pipeline_cache::run_cached_tir_pipeline(
                &mut ir.functions,
                crate::tir::pipeline_cache::TirPipelineRunOptions {
                    target_info: llvm_tti.clone(),
                    cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Llvm,
                    cache_dir: None,
                    process_externs: false,
                    verify_lir: false,
                    tir_dump: env_setting("TIR_DUMP").as_deref() == Some("1"),
                    tir_stats: env_setting("TIR_OPT_STATS").as_deref() == Some("1"),
                    progress_prefix: Some("MOLT_BACKEND(llvm)"),
                    resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
                },
                preprocess_backend_tir_input,
            )
            .cached_tir;

            //  Whole-program module phase (Tier-2 E1 inliner activation, LLVM)
            // This is the LLVM lane's parity point with native/wasm: it runs the
            // SAME `run_module_pipeline` (CallGraph  ModuleSummaries  bottom-up
            // E1 inliner  module-slot promotion  post-inline rebuild) the
            // Cranelift and WASM drivers run  but on the LLVM lane's own TIR
            // functions, with the LLVM cost model (`llvm_tti`), and it lowers the
            // resulting inlined `TirModule` *directly* to LLVM IR below. There is
            // NO SimpleIR round-trip on the LLVM path: the merged bodies stay in
            // TIR from the inliner straight through `try_lower_tir_to_llvm`. The
            // Cranelift-lane SimpleIR module phase above is skipped for `use_llvm`
            // (see the `!use_llvm` guard) so the program is inlined exactly once.
            //
            // Extern functions are runtime declarations with empty bodies (the
            // shared-stdlib partition's `stdlib_shared.o` symbols, already
            // externalized before this branch): they are not inlinable and stay
            // OUT of the module, so calls to them remain opaque call-graph edges
            // (exactly correct  an extern body is not owned by this object). They
            // are re-declared below for call resolution. Because externalization
            // has already physically removed their bodies, the module the inliner
            // sees contains only locally-owned bodies, so the `non_inlinable` set
            // is empty here (the native lane needs it only because its module
            // phase runs *before* externalization).
            let non_inlinable = std::collections::HashSet::new();
            let owned_tir_run =
                crate::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
                    &ir.functions,
                    &mut llvm_cached_tir,
                    crate::tir::pipeline_cache::TirOwnedModulePipelineOptions {
                        target_info: &llvm_tti,
                        module_name: "llvm_module",
                        non_inlinable: &non_inlinable,
                        missing_tir_context: "LLVM TIR cache runner",
                        mode: if self.skip_ir_passes {
                            crate::tir::pipeline_cache::TirOwnedModulePipelineMode::TerminalDropsOnly
                        } else {
                            crate::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase
                        },
                    },
                );
            let tir_funcs = owned_tir_run.tir_functions;

            llvm.function_return_types = tir_funcs
                .iter()
                .map(|(_, func)| (func.name.clone(), func.return_type.clone()))
                .collect();
            // Build LLVM representation facts from the exact post-module-phase
            // TIR the LLVM backend is about to lower. The trusted-unbox gate is
            // pure TIR/value-range authority, so fresh ValueIds introduced by
            // inlining are classified from the merged body itself.
            //
            // The table remains keyed by NAME because the module phase reorders
            // functions (externs first) and can grow caller bodies; consumers
            // look up by the final function name, never by pre-inline position.
            llvm.function_repr_facts = tir_funcs
                .iter()
                .filter(|(is_extern, _)| !*is_extern)
                .map(|(_, tir_func)| {
                    (
                        tir_func.name.clone(),
                        crate::representation_plan::LlvmReprFacts::build(tir_func),
                    )
                })
                .collect();

            // Parameter ABI carriers, derived from the SAME repr facts the body
            // lowers against: an unprovable-range `int` param is carried `DynBox`
            // (boxed), a value-range-proven one stays raw `I64`. This is the
            // caller-side coercion target that must agree with the callee's
            // entry-param carrier (`FunctionLowering::effective_block_arg_type`);
            // deriving both from `effective_param_types` over the same
            // `repr_by_value` keeps a heap-BigInt argument boxed end to end
            // (the trusted-unbox truncation bug-class is un-creatable at the call
            // boundary). Externs (no repr facts  they are opaque runtime
            // declarations) keep their declared ABI param types.
            llvm.function_param_types = tir_funcs
                .iter()
                .map(|(is_extern, tir_func)| {
                    let tys = if *is_extern {
                        tir_func.param_types.clone()
                    } else {
                        match llvm.function_repr_facts.get(&tir_func.name) {
                            Some(facts) => facts.effective_param_types(tir_func),
                            None => tir_func.param_types.clone(),
                        }
                    };
                    (tir_func.name.clone(), tys)
                })
                .collect();

            for (_, tir_func) in &tir_funcs {
                crate::llvm_backend::lowering::declare_tir_function(tir_func, &llvm);
            }

            for (is_extern, tir_func) in &tir_funcs {
                if *is_extern {
                    continue;
                }
                if env_setting("TIR_DUMP").as_deref() == Some("1")
                    || env_setting("MOLT_TIR_DUMP").as_deref() == Some("1")
                {
                    eprintln!(
                        "[LLVM] TIR for '{}':\n{}",
                        tir_func.name,
                        crate::tir::printer::print_function(tir_func)
                    );
                }
                crate::llvm_backend::lowering::try_lower_tir_to_llvm(tir_func, &llvm)
                    .unwrap_or_else(|err| panic!("{err}"));
            }

            //  Per-app intrinsic resolver
            // The LLVM-compiled application object must carry
            // `molt_app_resolve_intrinsic` (referenced by the CLI's main stub and
            // registered with the runtime before `molt_runtime_init`) exactly
            // like the Cranelift object. Emitted into the LLVM module here, after
            // every function is lowered, so the manifest intrinsics already exist
            // as declarations whose addresses the resolver table takes. Gated on
            // `emit_resolver_here` so batch/stdlib-cache LLVM objects (which set
            // `emit_app_intrinsic_resolver = false`) never emit a duplicate
            // `_molt_app_resolve_intrinsic` symbol.
            if emit_resolver_here {
                llvm.emit_app_resolver_function(&app_intrinsic_manifest);
            }

            // Dump LLVM IR under the repo-local debug artifact root when
            // MOLT_LLVM_DUMP_IR=1.
            let dump_ir = env_setting("MOLT_LLVM_DUMP_IR").as_deref() == Some("1");
            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/before_opt.ll",
                    llvm.dump_ir(),
                );
            }

            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed before optimization:\n{}",
                    msg.to_string()
                )
            });

            llvm.optimize(MoltOptLevel::Aggressive)
                .unwrap_or_else(|err| panic!("{err}"));
            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed after optimization:\n{}",
                    msg.to_string()
                )
            });

            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/after_opt.ll",
                    llvm.dump_ir(),
                );
            }

            if timing {
                let codegen_elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: LLVM codegen + optimization took {codegen_elapsed:.2?}"
                );
            }

            let tmp_obj = crate::debug_artifacts::prepare_unique_debug_artifact_path(
                "llvm/molt_llvm_output.o",
            )
            .expect("failed to prepare LLVM object path");
            llvm.emit_object(&tmp_obj, MoltOptLevel::Aggressive)
                .expect("LLVM object emission failed");
            let bytes = std::fs::read(&tmp_obj).unwrap_or_else(|err| {
                panic!(
                    "failed to read LLVM object file at {}: {}",
                    tmp_obj.display(),
                    err
                )
            });
            let _ = std::fs::remove_file(&tmp_obj);

            if timing {
                let total_elapsed = compile_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: total LLVM backend compile: {total_elapsed:.2?}                      ({func_count} functions, {total_ops} ops, {} bytes)",
                    bytes.len()
                );
            }

            return CompileOutput { bytes };
        }
        // Re-analyze after dead function elimination and megafunction
        // splitting so defined_functions/closure_functions reflect only the
        // surviving (and newly created chunk) functions. The leaf set is
        // consumed by codegen (recursion-guard skip). When a whole-program
        // module context is already set (the batched path), its leaf set wins
        // over this per-batch one (see `effective_leaf_functions` below), so
        // skip the redundant per-batch whole-program leaf lift here.
        let need_local_leaves = self.module_context.is_none();
        let mut ir_analysis = analyze_native_backend_ir(&ir, need_local_leaves);
        // Merge pre-split task annotations: megafunction splitting can
        // separate `func_new` from `set_attr_generic_obj(__molt_is_generator__)`
        // into different chunk functions, causing the post-split analysis to
        // miss generator/coroutine annotations.  The pre-split analysis
        // captured these correctly before the ops were split apart.
        for (name, kind) in &pre_split_task_kinds {
            ir_analysis.task_kinds.entry(name.clone()).or_insert(*kind);
        }
        for (name, size) in &pre_split_task_closure_sizes {
            ir_analysis
                .task_closure_sizes
                .entry(name.clone())
                .or_insert(*size);
        }
        // Conditional trace elimination: skip emitting trace_enter/trace_exit calls
        // when tracing is disabled. Each guarded call site emits 2 trace function calls
        // (enter + exit); eliminating them saves codegen work on cache misses and
        // keeps the default native backend lane focused on production semantics.
        // Trace emission is opt-in via MOLT_BACKEND_EMIT_TRACES=1.
        let emit_traces = env_setting("MOLT_BACKEND_EMIT_TRACES")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        // Compile functions into one module. Backend codegen failures are hard
        // failures: the compiler must not produce partial objects with
        // runtime-aborting placeholders for functions it could not compile.
        // Register extern functions (bodies in stdlib_shared.o) so the
        // backend declares them as Import linkage, resolved by the linker.
        for func in &ir.functions {
            if func.is_extern {
                self.external_function_names.insert(func.name.clone());
            }
        }
        // Filter out extern functions  they have no ops to compile.
        ir.functions.retain(|f| !f.is_extern);
        let func_count = ir.functions.len();
        let total_ops: usize = ir.functions.iter().map(|f| f.ops.len()).sum();
        eprintln!("MOLT_BACKEND: compiling {func_count} functions ({total_ops} total ops)");
        let codegen_start = std::time::Instant::now();
        let local_function_arities: BTreeMap<String, usize> = ir
            .functions
            .iter()
            .map(|func| (func.name.clone(), func.params.len()))
            .collect();
        let local_return_alias_summaries =
            crate::passes::compute_return_alias_summaries(&ir.functions);
        let module_context = self.module_context.clone();
        let effective_function_arities =
            merge_function_arities(module_context.as_ref(), local_function_arities);
        // UNION the module context's whole-program metadata with this batch's
        // LOCAL scan (design-20 finding #3C activation): a `module_context` that
        // was built from a DIFFERENT function set (the stdlib cache) does not
        // contain a closure/task/leaf defined only in this batch. Replacing the
        // local scan dropped those, so a `call_guarded` to a user closure
        // skipped env extraction  garbage closure  subscript TypeError. Mirror
        // the union that `merge_function_arities`/`merge_function_has_ret`
        // already do; the local scan is authoritative for this batch's own
        // definitions, the module context adds cross-batch knowledge.
        let effective_closure_functions = merge_closure_functions(
            module_context.as_ref(),
            ir_analysis.closure_functions.clone(),
        );
        let effective_task_kinds =
            merge_task_kinds(module_context.as_ref(), ir_analysis.task_kinds.clone());
        let effective_task_closure_sizes = merge_task_closure_sizes(
            module_context.as_ref(),
            ir_analysis.task_closure_sizes.clone(),
        );
        let effective_leaf_functions =
            merge_leaf_functions(module_context.as_ref(), ir_analysis.leaf_functions.clone());
        // UNION (same rationale as merge_closure_functions): a module context
        // built from a different function set does not carry THIS batch's own
        // return-alias summaries; the local computation must not be dropped, or a
        // caller in this batch loses the callee's RC-return contract. Local wins
        // on overlap (it is recomputed over the post-optimization bodies).
        let effective_return_alias_summaries = {
            let mut merged = module_context
                .as_ref()
                .map(|context| context.return_alias_summaries.clone())
                .unwrap_or_default();
            merged.extend(local_return_alias_summaries);
            merged
        };
        let local_function_has_ret = compute_function_has_ret(&ir.functions);
        let effective_function_has_ret =
            merge_function_has_ret(module_context.as_ref(), local_function_has_ret);
        let mut module_known_functions = ir_analysis.defined_functions.clone();
        module_known_functions.extend(self.external_function_names.iter().cloned());
        let mut compiled = 0u32;
        let failed = 0u32;
        let mut slowest_func: Option<(String, std::time::Duration)> = None;
        // Progress reporting: pick interval based on function count so the
        // user sees roughly 20 updates during a long build, but at least
        // every 50 functions.
        let progress_interval = (func_count / 20).clamp(1, 50);
        let mut last_progress = std::time::Instant::now();
        let mut deferred_codegen_ops = 0usize;

        for mut func_ir in ir.functions {
            let func_name = func_ir.name.clone();
            let func_op_count = func_ir.ops.len().max(1);
            // Fuse `obj.method(args)` (get_attr_generic_ptr + callargs +
            // call_bind) into a single allocation-free `call_method_ic` op
            // (CPython LOAD_METHOD/CALL_METHOD optimisation).  Run as the LAST
            // transformation before codegen. TIR has first-class IC opcodes, but
            // this backend consumes the final SimpleIR stream, so the fused ops
            // must not re-enter the TIR roundtrip or the whole-program leaf/alias
            // analyses (all already complete).
            fuse_method_dispatch(&mut func_ir);
            let func_start = std::time::Instant::now();
            self.compile_func(
                func_ir,
                &effective_task_kinds,
                &effective_task_closure_sizes,
                &ir_analysis.defined_functions,
                &module_known_functions,
                &effective_closure_functions,
                &effective_return_alias_summaries,
                emit_traces,
                &effective_leaf_functions,
                &effective_function_arities,
                &effective_function_has_ret,
            );
            let func_elapsed = func_start.elapsed();
            if timing && func_elapsed.as_millis() > 500 {
                eprintln!("MOLT_BACKEND_TIMING: function `{func_name}` took {func_elapsed:.2?}");
            }
            if slowest_func.as_ref().is_none_or(|(_, d)| func_elapsed > *d) {
                slowest_func = Some((func_name, func_elapsed));
            }
            deferred_codegen_ops = deferred_codegen_ops.saturating_add(func_op_count);
            if should_flush_deferred_codegen(self.deferred_defines.len(), deferred_codegen_ops) {
                let deferred_count = self.deferred_defines.len();
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: bounded Cranelift flush ({deferred_count} functions, {deferred_codegen_ops} source ops) took {flush_elapsed:.2?}"
                    );
                }
                deferred_codegen_ops = 0;
            }
            compiled += 1;
            // Print progress at regular intervals, or every 500ms for
            // slow builds where individual functions take a long time.
            if (compiled as usize).is_multiple_of(progress_interval)
                || last_progress.elapsed().as_millis() >= 500
            {
                let pct = (compiled as f64 / func_count as f64 * 100.0) as u32;
                let elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: [{pct:3}%] compiled {compiled}/{func_count} functions ({elapsed:.1?} elapsed)"
                );
                last_progress = std::time::Instant::now();
            }
        }
        if timing {
            let codegen_elapsed = codegen_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: Cranelift codegen took {codegen_elapsed:.2?}");
            if let Some((name, dur)) = &slowest_func {
                eprintln!("MOLT_BACKEND_TIMING: slowest function: `{name}` ({dur:.2?})");
            }
        }
        debug_assert_eq!(failed, 0, "native backend no longer soft-fails functions");
        //  Parallel Cranelift compilation
        // All functions were IR-built sequentially above (declarations
        // and Cranelift IR construction are not thread-safe), but actual
        // machine-code compilation (register allocation, instruction
        // selection, encoding) is deferred.  Flush them now in parallel.
        {
            let deferred_count = self.deferred_defines.len();
            if deferred_count > 0 {
                let deferred_ops = deferred_codegen_ops;
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: final Cranelift flush ({deferred_count} functions, {deferred_ops} source ops) took {flush_elapsed:.2?}"
                    );
                }
            }
        }
        //  Per-app intrinsic resolver
        // Emit `molt_app_resolve_intrinsic` AFTER the main flush so every
        // intrinsic FuncId created by a direct call already exists in the module
        // (reused via `get_name`); only manifest intrinsics are address-taken
        // here. The main stub registers this resolver before `molt_runtime_init`,
        // so the runtime resolves intrinsics through it instead of the
        // staticlib's `resolve_symbol`, keeping `resolve_symbol`/
        // `resolve_core_symbol` native-unreachable for dead-stripping.
        //
        // Emit it ONCE per final binary, into the designated main application
        // object (`emit_app_intrinsic_resolver`). Stdlib-cache batch objects and
        // all-but-one program batch set this `false`, so there is no duplicate
        // `_molt_app_resolve_intrinsic` symbol at link. The threaded manifest
        // covers every name-resolved intrinsic across all objects, including
        // stdlib wrappers compiled into the separate stdlib cache object.
        if emit_resolver_here {
            self.emit_app_resolver_function(&app_intrinsic_manifest);
        }
        //  Post-compilation: fail closed on declared-but-undefined exports.
        // These are always backend contract violations: either a call site
        // declared an impossible overload, a function was skipped, or codegen
        // failed to define a body.
        let mut undefined_exports = Vec::new();
        let declared: Vec<(String, cranelift_codegen::ir::Signature)> = self
            .module
            .declarations()
            .get_functions()
            .filter_map(|(_fid, decl)| {
                let name = decl.name.clone()?;
                if decl.linkage == cranelift_module::Linkage::Export
                    && !self.defined_func_names.contains(&name)
                {
                    Some((name, decl.signature.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (name, sig) in declared {
            // In batched compilation, functions that exist in another batch
            // are valid imports for the linker to resolve at merge time.
            if !self.external_function_names.is_empty()
                && self.external_function_names.contains(&name)
            {
                self.module
                    .declare_function(&name, cranelift_module::Linkage::Import, &sig)
                    .unwrap_or_else(|err| {
                        panic!("failed to mark cross-batch function `{name}` as import: {err}")
                    });
                continue;
            }
            undefined_exports.push(name);
        }
        if !undefined_exports.is_empty() {
            undefined_exports.sort();
            panic!(
                "native backend left {} exported function declaration(s) undefined: {}",
                undefined_exports.len(),
                undefined_exports.join(", ")
            );
        }

        let emit_start = std::time::Instant::now();
        let SimpleBackend { module, .. } = self;
        #[cfg(target_os = "macos")]
        let mut product = module.finish();
        #[cfg(not(target_os = "macos"))]
        let product = module.finish();
        // Set MachO platform load command so ld doesn't emit
        // "no platform load command found" warnings on macOS.
        #[cfg(target_os = "macos")]
        {
            use cranelift_object::object::write::MachOBuildVersion;
            // Encode macOS 11.0.0 as minimum deployment target.
            // Version encoding: xxxx.yy.zz nibbles => 0x000B0000 = 11.0.0
            let mut bv = MachOBuildVersion::default();
            bv.platform = cranelift_object::object::macho::PLATFORM_MACOS;
            bv.minos = 0x000B_0000; // macOS 11.0.0
            bv.sdk = 0; // no SDK constraint
            product.object.set_macho_build_version(bv);
        }
        let bytes = product.emit().unwrap();
        if timing {
            let emit_elapsed = emit_start.elapsed();
            let total_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: object emit took {emit_elapsed:.2?}");
            eprintln!(
                "MOLT_BACKEND_TIMING: total backend compile: {total_elapsed:.2?} \
                 ({func_count} functions, {total_ops} ops, {} bytes)",
                bytes.len()
            );
        }
        CompileOutput { bytes }
    }
}
