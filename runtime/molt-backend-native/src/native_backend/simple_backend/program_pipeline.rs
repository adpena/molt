use super::*;
use molt_tir::ir_rewrites::{elide_useless_try_blocks_for_function, rewrite_phi_to_store_load};
use std::fmt::Write as _;

pub(in crate::native_backend::simple_backend) struct NativeProgramPipeline {
    pub(in crate::native_backend::simple_backend) emit_resolver_here: bool,
    pub(in crate::native_backend::simple_backend) app_callable_manifest: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) source_callables:
        molt_tir::trampolines::CallableMetadata,
    pub(in crate::native_backend::simple_backend) final_functions: Option<NativeFinalFunctions>,
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
        // Import bedrock: module init bodies are reachable only through the
        // registry blob's MODULE_INIT_TABLE relocations (no call edges), so
        // the registry's init symbols are dead-function-elimination roots.
        let module_registry_roots: std::collections::BTreeSet<String> = self
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        // Whole-program reachability is the first backend custody boundary for
        // app objects. The frontend/stdlib transport may carry the full
        // importable stdlib graph, but TIR optimization and codegen must only
        // see functions reachable from declared roots. A second DFE after the
        // module inliner below catches functions made unreachable by inlining.
        if !self.skip_ir_passes {
            eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        }
        // Constructor task facts are immutable typed fields. Capture every
        // surviving definition and merge the retained source facts after
        // partitioning; physical symbol spelling carries no authority.
        let source_callables =
            molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions);
        // One lane target answers every target query below: the shared SimpleIR
        // pipeline, the Cranelift TIR cache, and dead-op elimination.
        let lane_target_info = if use_llvm {
            crate::tir::target_info::TargetInfo::llvm_release_fast()
        } else {
            crate::tir::target_info::TargetInfo::native_from_simd_caps(self.target_simd_caps())
        };
        molt_tir::passes::run_simple_ir_pipeline(ir, &lane_target_info);

        // Capture task/closure custody from the unsplit body exactly once.
        // The first TIR lift must see bounded bodies, including the LLVM lane.
        self.partition_sources.extend(split_megafunctions(ir));

        detect_gpu_kernels(&ir.functions);
        dump_raw_ir_if_requested(&ir.functions);

        let native_tti = (!use_llvm).then(|| lane_target_info.clone());
        let mut native_cached_tir = native_tti.as_ref().map(|native_tti| {
            crate::tir::pipeline_cache::run_cached_tir_pipeline(
                crate::tir::pipeline_cache::TirPipelineInput::SimpleIr(&mut ir.functions),
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
                |function| preprocess_backend_tir_input(function, native_tti),
            )
            .cached_tir
        });
        if use_llvm && !self.skip_ir_passes {
            eliminate_dead_ops(ir, &lane_target_info);
        }

        // Bound per-function optimization growth while transport allocations
        // can still be planned by the shared terminal ownership phase.
        if let (Some(target), Some(cached)) = (native_tti.as_ref(), native_cached_tir.as_mut()) {
            self.partition_sources.extend(
                crate::tir::pipeline_cache::partition_cached_functions_before_drops(
                    ir,
                    cached,
                    target,
                    split_megafunctions,
                ),
            );
        }

        let mut final_functions = if use_llvm {
            None
        } else {
            let native_tti = native_tti.as_ref().expect("native target missing");
            let non_inlinable: HashSet<String> = if self.skip_shared_stdlib_partition {
                BTreeSet::new()
            } else {
                shared_stdlib_external_symbols(ir, &self.partition_sources)
            }
            .into_iter()
            .collect();
            let typed = crate::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
                &ir.functions,
                native_cached_tir
                    .as_mut()
                    .expect("native finalization requires typed custody"),
                crate::tir::pipeline_cache::TirOwnedModulePipelineOptions {
                    target_info: native_tti,
                    module_name: "native_module",
                    non_inlinable: &non_inlinable,
                    missing_tir_context: "native final function custody",
                    mode: if self.skip_ir_passes {
                        crate::tir::pipeline_cache::TirOwnedModulePipelineMode::TerminalDropsOnly
                    } else {
                        crate::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase
                    },
                },
            );
            let mut final_functions = NativeFinalFunctions::from_final_tir(
                SimpleIR {
                    functions: std::mem::take(&mut ir.functions),
                    profile: ir.profile.take(),
                },
                typed,
            );
            if !self.skip_ir_passes {
                final_functions.retain_reachable(&module_registry_roots);
            }
            Some(final_functions)
        };
        if use_llvm && !self.skip_ir_passes {
            eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        }
        let final_ir = final_functions
            .as_ref()
            .map_or(&*ir, NativeFinalFunctions::ir);
        if !use_llvm {
            dump_rewritten_ir_if_requested(&final_ir.functions);
        }

        let emit_resolver_here = self.emit_app_callable_resolver;
        let app_callable_manifest = if emit_resolver_here {
            self.app_callable_manifest.take().unwrap_or_else(|| {
                // `_checked`: requires the staticlib symbol set (fail-closed)
                // only when some `molt_`-prefixed callable candidate exists. An
                // empty module has a necessarily empty manifest and must not
                // demand a symbol file that is not staged for it.
                crate::passes::compute_app_callable_manifest_checked(&final_ir.functions)
            })
        } else {
            self.app_callable_manifest.take().unwrap_or_default()
        };
        if !self.skip_shared_stdlib_partition {
            if let Some(final_functions) = &mut final_functions {
                final_functions.externalize_partition(
                    self.skip_ir_passes,
                    &module_registry_roots,
                    &self.partition_sources,
                );
            } else {
                externalize_shared_stdlib_partition(
                    ir,
                    &module_registry_roots,
                    &self.partition_sources,
                );
            }
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }

        NativeProgramPipeline {
            emit_resolver_here,
            app_callable_manifest,
            source_callables,
            final_functions,
        }
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

fn dump_rewritten_ir_if_requested(functions: &[FunctionIR]) {
    let Ok(requested) = std::env::var("MOLT_DUMP_REWRITTEN_FUNC") else {
        return;
    };
    if let Some(function) = functions.iter().find(|function| function.name == requested) {
        dump_rewritten_ops(&function.ops);
    }
}

#[cfg(feature = "llvm")]
pub(super) fn dump_rewritten_tir_if_requested(functions: &[(bool, crate::tir::TirFunction)]) {
    let Ok(requested) = std::env::var("MOLT_DUMP_REWRITTEN_FUNC") else {
        return;
    };
    if let Some((_, function)) = functions
        .iter()
        .find(|(_, function)| function.name == requested)
    {
        // A requested diagnostic is the only LLVM consumer of this projection;
        // normal machine emission keeps the final typed body as its sole owner.
        dump_rewritten_ops(&crate::tir::lower_to_simple::lower_to_simple_ir(function));
    }
}

fn dump_rewritten_ops(ops: &[OpIR]) {
    let mut dump = String::new();
    for (idx, op) in ops.iter().enumerate() {
        let _ = writeln!(dump, "{idx:04}: {:?}", op);
    }
    let _ = std::fs::write("tmp/rewritten_func_ir.txt", dump);
}

pub(crate) fn preprocess_backend_tir_input(
    tmp_func: &mut FunctionIR,
    target_info: &crate::tir::target_info::TargetInfo,
) {
    if tmp_func.ops.iter().any(|op| op.kind == "phi") {
        rewrite_phi_to_store_load(&mut tmp_func.ops);
        crate::tir::pipeline_cache::trace_tir_function_stage(
            &tmp_func.name,
            "after_phi_rewrite",
            tmp_func.ops.len(),
        );
    }
    if tmp_func.ops.iter().any(|op| op.kind == "exception_push") {
        elide_useless_try_blocks_for_function(tmp_func, target_info);
        crate::tir::pipeline_cache::trace_tir_function_stage(
            &tmp_func.name,
            "after_try_elision",
            tmp_func.ops.len(),
        );
    }
}
