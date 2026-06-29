use super::*;
use molt_tir::ir_rewrites::{
    elide_useless_try_blocks_for_function, rewrite_annotate_stubs, rewrite_copy_aliases,
    rewrite_phi_to_store_load,
};
use std::fmt::Write as _;

pub(in crate::native_backend::simple_backend) struct NativeProgramPipeline {
    pub(in crate::native_backend::simple_backend) emit_resolver_here: bool,
    pub(in crate::native_backend::simple_backend) app_intrinsic_manifest: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) pre_split_task_kinds:
        BTreeMap<String, TrampolineKind>,
    pub(in crate::native_backend::simple_backend) pre_split_task_closure_sizes:
        BTreeMap<String, i64>,
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(in crate::native_backend::simple_backend) fn prepare_program_for_codegen(
        &mut self,
        ir: &mut SimpleIR,
        use_llvm: bool,
        timing: bool,
        compile_start: &std::time::Instant,
    ) -> NativeProgramPipeline {
        apply_profile_order(ir);
        // Whole-program reachability is the first backend custody boundary for
        // app objects. The frontend/stdlib transport may carry the full
        // importable stdlib graph, but TIR optimization and codegen must only
        // see functions reachable from declared roots. A second DFE after the
        // module inliner below catches functions made unreachable by inlining.
        if !self.skip_ir_passes {
            eliminate_dead_functions(ir);
        }
        // Pre-TIR IR passes (parallel). Each pass operates on a single
        // FunctionIR with no shared mutable state, so all passes can run in
        // parallel across functions. Fusing them into one par_iter_mut avoids
        // repeated thread-pool dispatch overhead and keeps each function hot.
        {
            use rayon::prelude::*;
            ir.functions.par_iter_mut().for_each(|func_ir| {
                rewrite_stateful_loops(func_ir);
                // Eliminate UnboundLocalError checks early; they are dead code
                // in type-annotated functions and removing them before other
                // passes prevents the variable-access pattern from polluting
                // escape analysis and constant folding.
                eliminate_unbound_local_checks(func_ir);
                eliminate_redundant_guard_tags(func_ir);
                elide_dead_struct_allocs(func_ir);
                escape_analysis(func_ir);
                // rc_coalescing has its own MOLT_DISABLE_RC_COALESCE
                // early-return gate: single source of truth, no parallel
                // pre-check.
                rc_coalescing(func_ir);
                fold_constants(&mut func_ir.ops);
                fold_constants_cross_block(&mut func_ir.ops);
                elide_safe_exception_checks(func_ir);
                hoist_loop_invariants(func_ir);
            });
        }

        detect_gpu_kernels(&ir.functions);
        dump_raw_ir_if_requested(&ir.functions);

        let native_tti = (!use_llvm).then(|| {
            crate::tir::target_info::TargetInfo::native_from_simd_caps(
                crate::tir::target_info::SimdCaps::detect_host(),
            )
        });
        let mut native_cached_tir = native_tti.as_ref().map(|native_tti| {
            crate::tir::pipeline_cache::run_cached_tir_pipeline(
                &mut ir.functions,
                crate::tir::pipeline_cache::TirPipelineRunOptions {
                    target_info: native_tti.clone(),
                    cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Native,
                    cache_dir: None,
                    process_externs: false,
                    verify_lir: true,
                    tir_dump: env_setting("TIR_DUMP").as_deref() == Some("1"),
                    tir_stats: env_setting("TIR_OPT_STATS").as_deref() == Some("1"),
                    progress_prefix: Some("MOLT_BACKEND"),
                    resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
                },
                preprocess_backend_tir_input,
            )
            .cached_tir
        });
        if !self.skip_ir_passes {
            eliminate_dead_ops(ir);
        }

        let (pre_split_task_kinds, pre_split_task_closure_sizes) = self
            .run_cranelift_module_pipeline_if_needed(
                ir,
                use_llvm,
                native_tti.as_ref(),
                native_cached_tir.as_mut(),
            );

        if self.skip_ir_passes && !use_llvm {
            let native_tti = native_tti
                .as_ref()
                .expect("native TIR target info missing for Cranelift skip path");
            crate::tir::pipeline_cache::finalize_simple_ir_drops_from_cached_tir(
                &mut ir.functions,
                native_tti,
                native_cached_tir
                    .as_mut()
                    .expect("native TIR custody missing for Cranelift drop finalization"),
            );
        }

        // Dead function elimination after inlining reduces both object size and
        // downstream linker work.
        if !self.skip_ir_passes {
            eliminate_dead_functions(ir);
        }
        split_megafunctions(ir);
        rewrite_annotate_stubs(ir);
        run_post_tir_simple_ir_rewrites(&mut ir.functions);

        let emit_resolver_here = self.emit_app_intrinsic_resolver;
        let app_intrinsic_manifest = if emit_resolver_here {
            self.app_intrinsic_manifest.take().unwrap_or_else(|| {
                // `_checked`: requires the staticlib symbol set (fail-closed)
                // only when some `molt_`-prefixed const_str exists. An empty
                // module has a necessarily empty manifest and must not demand a
                // symbol file that is not staged for it.
                crate::passes::compute_intrinsic_manifest_checked(&ir.functions)
            })
        } else {
            self.app_intrinsic_manifest.take().unwrap_or_default()
        };
        if !self.skip_shared_stdlib_partition {
            externalize_shared_stdlib_partition(ir);
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }

        NativeProgramPipeline {
            emit_resolver_here,
            app_intrinsic_manifest,
            pre_split_task_kinds,
            pre_split_task_closure_sizes,
        }
    }

    fn run_cranelift_module_pipeline_if_needed(
        &self,
        ir: &mut SimpleIR,
        use_llvm: bool,
        native_tti: Option<&crate::tir::target_info::TargetInfo>,
        native_cached_tir: Option<&mut crate::tir::pipeline_cache::CachedTirCustody>,
    ) -> (BTreeMap<String, TrampolineKind>, BTreeMap<String, i64>) {
        // This pre-split capture only reads task annotations; the leaf set is
        // recomputed post-split, so skip the heavier whole-program leaf analysis.
        let analysis = analyze_native_backend_ir(ir, /* compute_leaves */ false);
        let pre_split_task_kinds = analysis.task_kinds;
        let pre_split_task_closure_sizes = analysis.task_closure_sizes;

        // The SimpleIR-carrier module phase runs for the Cranelift path and is
        // skipped for the LLVM path. LLVM lowers from TIR directly and runs the
        // same module pipeline on its own TIR functions in the LLVM branch, so
        // every emitted program is inlined exactly once.
        if !self.skip_ir_passes && !use_llvm {
            let native_tti = native_tti.expect("native TIR target info missing for Cranelift path");
            // Shared-stdlib bodies externalized into `stdlib_shared.o` must stay
            // opaque to this app object. Compute this before externalization
            // clears their ops, from the same predicate it uses.
            let external_symbols = if self.skip_shared_stdlib_partition {
                BTreeSet::new()
            } else {
                shared_stdlib_external_symbols(ir)
            };
            let non_inlinable: HashSet<String> = external_symbols.into_iter().collect();
            let native_cached_tir = native_cached_tir
                .expect("native TIR custody missing for Cranelift module pipeline");
            let _module_run =
                crate::tir::pipeline_cache::run_simple_ir_module_pipeline_from_cached_tir(
                    &mut ir.functions,
                    native_cached_tir,
                    crate::tir::pipeline_cache::TirSimpleIrModulePipelineOptions {
                        target_info: native_tti,
                        module_name: "native_module",
                        non_inlinable: &non_inlinable,
                        missing_tir_context: "native TIR cache runner",
                        backconvert_context: "native TIR module pipeline",
                        stage_observer: None,
                    },
                );
        }

        (pre_split_task_kinds, pre_split_task_closure_sizes)
    }
}

fn detect_gpu_kernels(functions: &[FunctionIR]) {
    let mut gpu_kernel_names: Vec<String> = Vec::new();
    for func_ir in functions {
        let is_gpu = func_ir.ops.iter().any(|op| {
            matches!(
                op.kind.as_str(),
                "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" | "gpu_barrier"
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
}

fn dump_raw_ir_if_requested(functions: &[FunctionIR]) {
    let Some(pattern) = std::env::var("MOLT_DUMP_FUNC_IR").ok() else {
        return;
    };
    for func_ir in functions
        .iter()
        .filter(|func_ir| !func_ir.is_extern && func_ir.name.contains(pattern.as_str()))
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
                idx,
                op.kind,
                op.out.as_deref().unwrap_or(""),
                op.var.as_deref().unwrap_or(""),
                op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                op.value,
                op.s_value,
                op.fast_int,
                op.fast_float));
        }
        let _ = crate::debug_artifacts::write_debug_artifact(format!("ir/{sanitized}.txt"), dump);
    }
}

fn run_post_tir_simple_ir_rewrites(functions: &mut [FunctionIR]) {
    for func in functions {
        rewrite_copy_aliases(&mut func.ops);
        // Split-field read deforestation runs after copy-alias rewrite so
        // inlined len/ord_at consumers reference the field's canonical SSA name.
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
}

pub(crate) fn preprocess_backend_tir_input(tmp_func: &mut FunctionIR) {
    if tmp_func.ops.iter().any(|op| op.kind == "phi") {
        rewrite_phi_to_store_load(&mut tmp_func.ops);
        crate::tir::pipeline_cache::trace_tir_function_stage(
            &tmp_func.name,
            "after_phi_rewrite",
            tmp_func.ops.len(),
        );
    }
    if tmp_func.ops.iter().any(|op| op.kind == "exception_push") {
        elide_useless_try_blocks_for_function(tmp_func);
        crate::tir::pipeline_cache::trace_tir_function_stage(
            &tmp_func.name,
            "after_try_elision",
            tmp_func.ops.len(),
        );
    }
}
