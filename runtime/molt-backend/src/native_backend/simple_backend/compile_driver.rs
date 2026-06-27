use super::*;
use std::fmt::Write as _;

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
        apply_profile_order(&mut ir);
        // Whole-program reachability is the first backend custody boundary for
        // app objects. The frontend/stdlib transport may carry the full
        // importable stdlib graph, but TIR optimization and codegen must only
        // see functions reachable from declared roots. A second DFE after the
        // module inliner below catches functions made unreachable by inlining.
        if !self.skip_ir_passes {
            eliminate_dead_functions(&mut ir);
        }
        //  Pre-TIR IR passes (parallel)
        // Each pass operates on a single FunctionIR with no shared
        // mutable state, so all 8 passes can run in parallel across
        // functions using rayon.  Fusing them into one par_iter_mut
        // avoids 8- thread-pool dispatch overhead and improves cache
        // locality (each function stays hot while all passes run).
        {
            use rayon::prelude::*;
            ir.functions.par_iter_mut().for_each(|func_ir| {
                rewrite_stateful_loops(func_ir);
                // Eliminate UnboundLocalError checks early  they are dead
                // code in type-annotated functions and removing them before
                // other passes prevents the ~11 ops per variable-access
                // pattern from polluting escape analysis and constant folding.
                eliminate_unbound_local_checks(func_ir);
                eliminate_redundant_guard_tags(func_ir);
                elide_dead_struct_allocs(func_ir);
                escape_analysis(func_ir);
                // rc_coalescing has its own MOLT_DISABLE_RC_COALESCE early-return
                // gate  single source of truth, no parallel pre-check.
                rc_coalescing(func_ir);
                fold_constants(&mut func_ir.ops);
                fold_constants_cross_block(&mut func_ir.ops);
                elide_safe_exception_checks(func_ir);
                hoist_loop_invariants(func_ir);
            });
        }
        //  GPU kernel detection
        // Functions containing GPU intrinsic ops (gpu_thread_id, gpu_block_id,
        // etc.) are GPU kernels.  Flag them in metadata so the GPU pipeline can
        // handle them separately, but they still flow through the canonical
        // TIR/LIR pipeline like every other function.
        let mut gpu_kernel_names: Vec<String> = Vec::new();
        for func_ir in &ir.functions {
            let is_gpu = func_ir.ops.iter().any(|op| {
                matches!(
                    op.kind.as_str(),
                    "gpu_thread_id"
                        | "gpu_block_id"
                        | "gpu_block_dim"
                        | "gpu_grid_dim"
                        | "gpu_barrier"
                )
            });
            if is_gpu {
                gpu_kernel_names.push(func_ir.name.clone());
            }
        }
        if !gpu_kernel_names.is_empty() {
            eprintln!(
                "[molt-gpu] Detected {} GPU kernel function(s): {:?}",
                gpu_kernel_names.len(),
                gpu_kernel_names
            );
        }

        //  TIR optimization pipeline
        // The TIR roundtrip (lower->refine->optimize->lower-back) is mandatory
        // for backend-facing functions. Debugging must use dumps and verifier
        // evidence rather than bypassing typed IR.
        // All TIR-lowered control flow uses pure label/jump/br_if patterns
        // (no structured loop_start/loop_end).  The Cranelift function compiler
        // handles back-edges via has_loop_or_backedge detection.
        let mut optimized_tir_by_name: std::collections::BTreeMap<
            String,
            crate::tir::function::TirFunction,
        > = std::collections::BTreeMap::new();
        {
            use rayon::prelude::*;

            let _tir_dump = env_setting("TIR_DUMP").as_deref() == Some("1");
            let _tir_stats = env_setting("TIR_OPT_STATS").as_deref() == Some("1");
            let mut tir_cache =
                crate::tir::cache::CompilationCache::open(crate::tir::cache::backend_cache_dir());

            // Phase 1 (sequential): check cache for every function. For cache
            // hits, apply immediately. For misses, collect the function index,
            // content hash, and op count for bounded optimization batches.
            let mut work_items: Vec<TirOptimizationWorkItem> = Vec::new();

            // Debug: dump raw IR for functions matching MOLT_DUMP_FUNC_IR pattern.
            let dump_func_pattern = std::env::var("MOLT_DUMP_FUNC_IR").ok();

            for (i, func_ir) in ir.functions.iter_mut().enumerate() {
                // Extern functions: bodies live in stdlib_shared.o.
                // They are registered as external before codegen.
                if func_ir.is_extern {
                    continue;
                }

                // Dump raw ops to file for debugging TIR roundtrip issues.
                if let Some(ref pattern) = dump_func_pattern
                    && func_ir.name.contains(pattern.as_str())
                {
                    let sanitized: String = func_ir
                        .name
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let mut dump = String::new();
                    dump.push_str(&format!(
                        "// func: {} ({} ops)\n",
                        func_ir.name,
                        func_ir.ops.len()
                    ));
                    dump.push_str(&format!("// params: {:?}\n", func_ir.params));
                    dump.push_str(&format!("// param_types: {:?}\n", func_ir.param_types));
                    for (idx, op) in func_ir.ops.iter().enumerate() {
                        dump.push_str(&format!("{:4}: kind={:30} out={:20} var={:20} args={:40} val={:?} sval={:?} fi={:?} ff={:?}\n",
                                idx, op.kind,
                                op.out.as_deref().unwrap_or(""),
                                op.var.as_deref().unwrap_or(""),
                                op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                                op.value, op.s_value, op.fast_int, op.fast_float));
                    }
                    let _ = crate::debug_artifacts::write_debug_artifact(
                        format!("ir/{sanitized}.txt"),
                        dump,
                    );
                }

                // Loop markers (loop_start, loop_end) are now preserved through
                // the TIR roundtrip via LoopRole metadata on TirFunction, so
                // functions with loops benefit from TIR optimization.
                let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
                let cache_hash_body = native_tir_cache_hash_body(&body_bytes);
                let content_hash = crate::tir::cache::CompilationCache::compute_hash_with_signature(
                    &func_ir.name,
                    &func_ir.params,
                    func_ir.param_types.as_deref(),
                    &cache_hash_body,
                );
                // Check TIR cache: if we have validated optimized ops from a
                // previous build with the same content hash, reuse them.
                if let Some(cached_bytes) = tir_cache.get(&content_hash)
                    && let Some(cached_tir) =
                        crate::tir::serialize::deserialize_tir_function(&cached_bytes)
                {
                    let cached_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&cached_tir);
                    debug_assert!(
                        crate::tir::lower_to_simple::validate_labels(&cached_ops),
                        "native TIR cache back-conversion emitted invalid labels for '{}'",
                        cached_tir.name
                    );
                    func_ir.ops = cached_ops;
                    optimized_tir_by_name.insert(func_ir.name.clone(), cached_tir);
                    continue;
                }
                work_items.push(TirOptimizationWorkItem {
                    index: i,
                    content_hash,
                    op_count: func_ir.ops.len(),
                });
            }

            let uncached_count = work_items.len();
            if uncached_count > 0 {
                let resource_plan = tir_optimization_resource_plan();
                let work_batches = partition_tir_optimization_work_items_with_limits(
                    work_items,
                    resource_plan.wave_function_limit,
                    resource_plan.wave_op_budget,
                );
                let batch_count = work_batches.len();
                if batch_count == 1 {
                    eprintln!(
                        "MOLT_BACKEND: TIR optimizing {uncached_count} uncached functions with {} worker(s)",
                        resource_plan.threads
                    );
                } else {
                    eprintln!(
                        "MOLT_BACKEND: TIR optimizing {uncached_count} uncached functions in {batch_count} bounded waves with {} worker(s)",
                        resource_plan.threads
                    );
                }
                let tir_start = std::time::Instant::now();

                // Phase 2 (parallel): run the TIR pipeline on every uncached
                // function.  Each work item borrows only its own FunctionIR
                // (via index) and produces an independent result.
                //
                // We cannot borrow &mut ir.functions[i] in parallel because
                // Rust's borrow checker does not allow multiple mutable refs
                // into the same Vec, even at disjoint indices, through closures.
                // Instead we extract the ops, optimize them in parallel, and
                // write them back.
                // Each element: (func_index, content_hash, optimized_ops)
                // Use a custom thread pool with 16MB stacks for TIR.
                // lower_to_simple_ir has deeply nested closures capturing
                // many HashMaps, which exceeds rayon's default 8MB stacks.
                let tir_pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(resource_plan.threads)
                    .stack_size(64 * 1024 * 1024)
                    .build()
                    .expect("Failed to build TIR thread pool");
                for (batch_idx, batch_items) in work_batches.into_iter().enumerate() {
                    let batch_ops = batch_items.iter().map(|wi| wi.op_count).sum::<usize>();
                    if batch_count > 1 {
                        eprintln!(
                            "MOLT_BACKEND: TIR batch {}/{} ({} functions, {} ops / budget {})",
                            batch_idx + 1,
                            batch_count,
                            batch_items.len(),
                            batch_ops,
                            resource_plan.wave_op_budget
                        );
                    }
                    let inputs: Vec<TirOptimizationInput> = batch_items
                        .into_iter()
                        .map(|wi| {
                            let func_ir = &ir.functions[wi.index];
                            TirOptimizationInput {
                                index: wi.index,
                                content_hash: wi.content_hash,
                                name: func_ir.name.clone(),
                                params: func_ir.params.clone(),
                                ops: func_ir.ops.clone(),
                                param_types: func_ir.param_types.clone(),
                            }
                        })
                        .collect();
                    let results: Vec<TirOptimizationOutput> = tir_pool
                        .install(|| inputs.into_par_iter().map(optimize_tir_input).collect());

                    // Phase 3 (sequential): apply validated TIR ops and cache them.
                    for output in results {
                        let func_ir = &mut ir.functions[output.index];
                        func_ir.ops = output.simple_ops;
                        let bytes = crate::tir::serialize::serialize_tir_function(&output.tir_func);
                        tir_cache.put(&output.content_hash, &bytes, vec![]);
                        optimized_tir_by_name.insert(func_ir.name.clone(), output.tir_func);
                    }
                }

                let tir_elapsed = tir_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: TIR parallel optimization took {tir_elapsed:.2?} for {uncached_count} functions"
                );
            }

            tir_cache.save_index();
        }
        if !self.skip_ir_passes {
            eliminate_dead_ops(&mut ir);
        }
        // Post-TIR: analysis + inlining (from main)
        // Capture task_kinds and task_closure_sizes BEFORE megafunction splitting.
        // Megafunction splitting can separate `func_new` from its corresponding
        // `set_attr_generic_obj(__molt_is_generator__)` into different chunk
        // functions, which breaks the per-function cross-reference in
        // `analyze_native_backend_ir`.  By capturing generator/coroutine
        // annotations now, we ensure they survive the split.
        let pre_split_task_kinds: BTreeMap<String, TrampolineKind>;
        let pre_split_task_closure_sizes: BTreeMap<String, i64>;
        {
            // This pre-split capture only reads task annotations; the leaf set is
            // recomputed post-split below, so skip the (heavier) whole-program
            // leaf analysis here.
            let analysis = analyze_native_backend_ir(&ir, /* compute_leaves */ false);
            pre_split_task_kinds = analysis.task_kinds;
            pre_split_task_closure_sizes = analysis.task_closure_sizes;
            // The SimpleIR-carrier module phase runs for the Cranelift path
            // (gated on skip_ir_passes). It is SKIPPED for the LLVM path
            // (`use_llvm`): LLVM lowers from TIR directly, so it runs the SAME
            // `run_module_pipeline` on its own TIR functions inside the
            // `if use_llvm` branch below and lowers the inlined `TirModule`
            // *directly*  never round-tripping the merged bodies back through
            // SimpleIR. Running the module phase here too would inline twice (once
            // into the SimpleIR `ir.functions`, then again when the LLVM branch
            // re-inlines its TIR lift). One inliner per emitted program: the
            // SimpleIR carrier feeds Cranelift, the TIR module feeds LLVM.
            //
            // The deleted `needs_inlining` heuristic keyed on
            // `kind == "call_internal"`, but the TIR roundtrip's back-conversion
            // re-emits every call as `kind: "call"`  so the flag was always
            // false for TIR-processed functions and had silently disabled
            // production inlining (for the legacy SimpleIR inliner too). The
            // call graph itself is the authority on whether anything is
            // inlinable; an inline-free module just runs a cheap analysis.
            if !self.skip_ir_passes && !use_llvm {
                // E1 ACTIVATION: the TIR function inliner (tir/passes/inliner.rs,
                // via run_module_pipeline) is now the production inliner  SSA-based,
                // exception-label-safe, call-graph bottom-up, cost-model-gated, and
                // it re-optimizes each merged caller through the per-function
                // pipeline. It replaces the legacy SimpleIR `inline_functions`
                // (string-rename, no SSA, no cost model  retired in e-4).
                // Assemble the module from the optimized TIR custody map (fresh
                // worker output or native TIR cache hit) so module transforms and
                // terminal drops do not re-lift the expanded SimpleIR carrier.
                // Back-convert ONLY functions changed by module/drop phases;
                // every unchanged function keeps its byte-identical
                // per-function output.
                // Rollback: MOLT_DISABLE_INLINING=1 (guard in run_inliner).
                let native_tti = crate::tir::target_info::TargetInfo::native_from_simd_caps(
                    crate::tir::target_info::SimdCaps::detect_host(),
                );
                let mut tir_functions = Vec::new();
                let mut idx_map = Vec::new();
                for (idx, func_ir) in ir.functions.iter().enumerate() {
                    if func_ir.is_extern {
                        continue;
                    }
                    let tir_func = optimized_tir_by_name
                        .remove(&func_ir.name)
                        .unwrap_or_else(|| crate::tir::lower_from_simple::lower_to_tir(func_ir));
                    tir_functions.push(tir_func);
                    idx_map.push(idx);
                }
                let mut tir_module = crate::tir::function::TirModule {
                    name: "native_module".to_string(),
                    functions: tir_functions,
                };
                // Functions the shared-stdlib partition will externalize into
                // `stdlib_shared.o` have external linkage: the inliner must keep
                // the external reference rather than fork a private copy of a body
                // this app object does not own (computed BEFORE `externalize_*`
                // clears their ops, from the same predicate it uses).
                let external_symbols = if self.skip_shared_stdlib_partition {
                    BTreeSet::new()
                } else {
                    shared_stdlib_external_symbols(&ir)
                };
                let non_inlinable: std::collections::HashSet<String> =
                    external_symbols.into_iter().collect();
                let module_analysis =
                    crate::tir::run_module_pipeline(&mut tir_module, &native_tti, &non_inlinable);
                let changed: std::collections::HashSet<&str> = module_analysis
                    .changed_functions
                    .iter()
                    .map(String::as_str)
                    .collect();
                for (pos, &orig_idx) in idx_map.iter().enumerate() {
                    let tir_func = &tir_module.functions[pos];
                    if changed.contains(tir_func.name.as_str()) {
                        let ops = crate::tir::lower_to_simple::lower_to_simple_ir(tir_func);
                        debug_assert!(
                            crate::tir::lower_to_simple::validate_labels(&ops),
                            "E1: inlined back-conversion emitted invalid labels for '{}'",
                            tir_func.name
                        );
                        ir.functions[orig_idx].ops = ops;
                    }
                }
            }
        }
        //  RC drop insertion: terminal phase for the skip_ir_passes path
        // The whole-program module phase (which runs the drop finalizer over its
        // TIR module, see `run_module_pipeline`) is SKIPPED for `skip_ir_passes`
        // builds  the stdlib-cache object and the per-batch application codegen,
        // which forgo inlining/promotion and do per-function-only optimization.
        // Drop insertion is a per-function correctness concern (it closes the
        // expression-temporary leak), NOT a whole-program optimization, so it must
        // still run there. With no module phase, the (already-run, cached)
        // per-function pipeline is the last transform, so drops run here as the
        // terminal step  over the SimpleIR carrier, post-cache (the cache never
        // stores drop-inserted ops keyed by the drop-free input hash). Runs BEFORE
        // `split_megafunctions` so DecRef/value-def pairs stay within one function
        // (the non-skip path likewise drops in the module phase, before splitting).
        // The LLVM lane has its own module phase below and is excluded here.
        if self.skip_ir_passes && !use_llvm {
            let native_tti = crate::tir::target_info::TargetInfo::native_from_simd_caps(
                crate::tir::target_info::SimdCaps::detect_host(),
            );
            crate::tir::drop_phase::finalize_simple_ir_drops_with_tir_custody(
                &mut ir.functions,
                &native_tti,
                &mut optimized_tir_by_name,
            );
        }
        // Dead function elimination: remove functions that are unreachable from
        // the entry point after inlining.  This reduces code size for both the
        // native object and the downstream linker's work.
        if !self.skip_ir_passes {
            eliminate_dead_functions(&mut ir);
        }
        // Megafunction splitting: break up functions with >4000 ops (or
        // MOLT_MAX_FUNCTION_OPS) into private chunk functions to avoid
        // Cranelift's O(n^2) register allocator blowup.
        split_megafunctions(&mut ir);
        rewrite_annotate_stubs(&mut ir);
        for func in &mut ir.functions {
            rewrite_copy_aliases(&mut func.ops);
            // Split-field read deforestation: a non-escaping `s.split(sep)[idx]`
            // consumed only by `len`/`ord(field[i])`/`== const` (the shape the
            // split-field-enabled inliner exposes when it splices `parse_int(field)`)
            // is rewritten to bounds-once reads so the field never materializes
            // the csv/etl ETL release-blocker fix. Runs AFTER copy-alias rewrite so
            // the inlined `len`/`ord_at` consumers reference the field's canonical
            // SSA name directly (pre-collapse they read an alias of it).
            crate::passes::deforest_split_field_reads(func);
            canonicalize_direct_raise_edges(func);
            if std::env::var("MOLT_DUMP_REWRITTEN_FUNC").as_deref() == Ok(func.name.as_str()) {
                let mut dump = String::new();
                for (idx, op) in func.ops.iter().enumerate() {
                    let _ = writeln!(dump, "{idx:04}: {:?}", op);
                }
                let _ = std::fs::write("tmp/rewritten_func_ir.txt", dump);
            }
        }
        // Compute the per-app intrinsic manifest BEFORE the stdlib partition
        // clears extern function bodies. The stdlib_shared.o partition's
        // trampolines reach intrinsics by name too, and those uses must be
        // covered by the per-app resolver (RISK-3)  once `externalize_shared_stdlib_partition`
        // clears their ops, the manifest scan can no longer see them. The native
        // backend emits `molt_app_resolve_intrinsic` over exactly this set so
        // `resolve_symbol`/`resolve_core_symbol` become native-unreachable and
        // the linker dead-strips every unused intrinsic.
        // The manifest must cover every intrinsic reached via the dynamic
        // name-based resolver path across ALL objects of the final binary. When
        // the orchestrator split the program (stdlib cache split / batching) it
        // pre-computes the manifest over the full function set and threads it in;
        // otherwise this object holds the full set and we derive it locally.
        // Only the object that emits the resolver needs (and computes) the
        // manifest; stdlib-cache / non-primary batch objects set
        // `emit_app_intrinsic_resolver = false` and never reference it. Deriving it
        // there would also wrongly demand the staticlib symbol set for an object
        // that takes no intrinsic addresses. When the orchestrator split the
        // program it threads a pre-computed full-set manifest in; otherwise this
        // object holds the full set and derives it locally against the REQUIRED
        // staticlib symbol set (no heuristic fallback  see
        // `runtime_intrinsic_symbols_required`).
        // The per-app resolver is emitted by BOTH the Cranelift path below and
        // the LLVM path (`use_llvm`): the LLVM-compiled application object must
        // carry `molt_app_resolve_intrinsic` (referenced by the CLI's main stub)
        // exactly like the Cranelift object, or the link leaves it undefined and
        // every name-based intrinsic resolution fails at runtime. The manifest
        // scan is backend-independent (it reads `FunctionIR` const_str ops
        // against the linked staticlib's intrinsic symbol set), so compute it
        // and require the exact symbol set  whenever THIS object will emit the
        // resolver, regardless of which codegen backend produces the bytes.
        let emit_resolver_here = self.emit_app_intrinsic_resolver;
        let app_intrinsic_manifest = if emit_resolver_here {
            self.app_intrinsic_manifest.take().unwrap_or_else(|| {
                // `_checked`: requires the staticlib symbol set (fail-closed)
                // only when some `molt_`-prefixed const_str exists  an empty
                // module (the CLI's post-build feature probe) has a necessarily
                // empty manifest and must not demand a symbol file that is not
                // staged for it.
                crate::passes::compute_intrinsic_manifest_checked(&ir.functions)
            })
        } else {
            self.app_intrinsic_manifest.take().unwrap_or_default()
        };
        if !self.skip_shared_stdlib_partition {
            externalize_shared_stdlib_partition(&mut ir);
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }
        //  LLVM backend dispatch
        // When MOLT_BACKEND=llvm and the llvm feature is compiled in, route
        // through the LLVM backend instead of Cranelift.  Each function is
        // lifted to TIR, lowered to LLVM IR, then the whole module is
        // optimized and emitted as a native object file.
        #[cfg(feature = "llvm")]
        if use_llvm {
            use crate::llvm_backend::{LlvmBackend, MoltOptLevel};
            use crate::tir::lower_from_simple::lower_to_tir;

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
            let mut tir_funcs: Vec<(bool, crate::tir::function::TirFunction)> = ir
                .functions
                .iter()
                .map(|func| {
                    let mut tir_func = lower_to_tir(func);
                    // Extern functions (e.g. shared-stdlib-partition symbols
                    // externalized by `externalize_shared_stdlib_partition`) have
                    // had their bodies cleared: they are declaration-only and live
                    // in `stdlib_shared.o`. They lower to a bodyless TIR function
                    // (no blocks, hence no entry block), which would fail the TIR
                    // verifier the moment the optimization pipeline ran on it. They
                    // are *declared* below (`declare_tir_function`) for call
                    // resolution but never *defined*, so there is nothing to
                    // optimize. Mirror the Cranelift per-function pipeline, which
                    // skips extern functions for the same reason. Lower for the
                    // signature only.
                    if !func.is_extern {
                        // Run the full TIR optimization pipeline  same as Cranelift/WASM.
                        // Without this, all values stay DynBox and every operation
                        // dispatches through the runtime instead of emitting native ops.
                        crate::tir::type_refine::refine_types(&mut tir_func);
                        let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &llvm_tti);
                        crate::tir::type_refine::refine_types(&mut tir_func);
                    }
                    (func.is_extern, tir_func)
                })
                .collect();

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
            if !self.skip_ir_passes {
                use crate::tir::function::TirModule;

                let mut externs: Vec<crate::tir::function::TirFunction> = Vec::new();
                let mut module = TirModule {
                    name: "llvm_module".to_string(),
                    functions: Vec::new(),
                };
                for (is_extern, tir_func) in tir_funcs.into_iter() {
                    if is_extern {
                        externs.push(tir_func);
                    } else {
                        module.functions.push(tir_func);
                    }
                }

                // Inlines bottom-up and re-optimizes merged callers; leaves every
                // changed body fully type-refined (see `run_inliner`). Rollback:
                // MOLT_DISABLE_INLINING=1 (guard in run_inliner).
                let _module_analysis = crate::tir::run_module_pipeline(
                    &mut module,
                    &llvm_tti,
                    &std::collections::HashSet::new(),
                );

                // Reassemble the lowering list: extern declarations first, then
                // the merged non-extern bodies. Declaration and lowering order is
                // immaterial  LLVM resolves calls by name and functions lower
                // independently.
                tir_funcs = Vec::with_capacity(externs.len() + module.functions.len());
                tir_funcs.extend(externs.into_iter().map(|f| (true, f)));
                tir_funcs.extend(module.functions.into_iter().map(|f| (false, f)));
            } else {
                // skip_ir_passes (LLVM batched / stdlib-cache path): the
                // whole-program module phase  which runs the terminal drop
                // finalizer over its TIR module  is skipped. Drop insertion is a
                // per-function correctness concern, so it still runs here on the
                // per-function-pipeline output, the last transform in this mode.
                // (The non-skip branch above ran drops inside `run_module_pipeline`.)
                // Funnels through the same `finalize_function_drops` entry as the
                // module/SimpleIR finalizers (uniform refine + double-process guard).
                for (is_extern, tir_func) in tir_funcs.iter_mut() {
                    if !*is_extern {
                        let _ =
                            crate::tir::drop_phase::finalize_function_drops(tir_func, &llvm_tti);
                    }
                }
            }

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
        if self.emit_app_intrinsic_resolver {
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
