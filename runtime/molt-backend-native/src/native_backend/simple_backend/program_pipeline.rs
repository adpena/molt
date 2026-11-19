use super::*;
use molt_ir::OpKind;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use molt_tir::ir_rewrites::rewrite_phi_to_store_load;
use std::fmt::Write as _;

pub(in crate::native_backend::simple_backend) struct NativeProgramPipeline {
    pub(in crate::native_backend::simple_backend) emit_resolver_here: bool,
    pub(in crate::native_backend::simple_backend) app_callable_manifest:
        crate::passes::AppCallableManifest,
    pub(in crate::native_backend::simple_backend) final_functions: Option<NativeFinalFunctions>,
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    fn take_app_callable_manifest(&mut self, ir: &SimpleIR) -> crate::passes::AppCallableManifest {
        if self.emit_app_callable_resolver {
            self.app_callable_manifest.take().unwrap_or_else(|| {
                crate::passes::compute_app_callable_manifest_checked(&ir.functions)
            })
        } else {
            self.app_callable_manifest.take().unwrap_or_default()
        }
    }

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
        let mut module_registry_roots: std::collections::BTreeSet<String> = self
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        // Whole-program reachability is the first backend custody boundary for
        // app objects. The frontend/stdlib transport may carry the full
        // importable stdlib graph, but TIR optimization and codegen must only
        // see functions reachable from declared roots. A second DFE after the
        // module inliner below catches functions made unreachable by inlining.
        // Per-step timing under MOLT_BACKEND_TIMING: where an edit's IR passes go.
        let mut step = std::time::Instant::now();
        let mut mark = |label: &str, functions: usize| {
            if timing {
                eprintln!(
                    "MOLT_BACKEND_TIMING:   {label} took {:.2?} ({functions} functions)",
                    step.elapsed()
                );
            }
            step = std::time::Instant::now();
        };
        if let Some(manifest) = &self.app_callable_manifest {
            module_registry_roots.extend(manifest.compiled_bodies.iter().cloned());
        }
        if !self.skip_ir_passes {
            eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        }
        mark("dead-function elimination", ir.functions.len());
        // Validate source constructor agreement before any partition can hide a conflict.
        molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions);
        // Inlining may erase a descriptor's publisher or move a lookup into
        // another body. Capture app resolver and embedded GPU identities on
        // the reachable source graph, where their transport is authoritative.
        let native_manifest = (!use_llvm).then(|| {
            let manifest = self.take_app_callable_manifest(ir);
            if self.emit_app_callable_resolver {
                module_registry_roots.extend(manifest.compiled_bodies.iter().cloned());
            } else {
                for function in &ir.functions {
                    molt_tir::passes::visit_gpu_descriptor_body_symbols(function, |symbol| {
                        module_registry_roots.insert(symbol.to_owned());
                    });
                }
            }
            manifest
        });
        // One lane target answers every target query below: the Cranelift TIR
        // cache. The TIR pipeline is the one optimizer.
        let lane_target_info = if use_llvm {
            crate::tir::target_info::TargetInfo::llvm_release_fast()
        } else {
            crate::tir::target_info::TargetInfo::native_from_simd_caps(self.target_simd_caps())
        };

        // Capture task/closure custody from the unsplit body exactly once.
        // The first TIR lift must see bounded bodies, including the LLVM lane.
        mark("callable metadata", ir.functions.len());
        split_megafunctions(ir);
        mark("megafunction split", ir.functions.len());

        detect_gpu_kernels(&ir.functions);
        dump_raw_ir_if_requested(&ir.functions);

        let native_tti = (!use_llvm).then(|| lane_target_info.clone());
        let mut native_cached_tir = native_tti.as_ref().map(|native_tti| {
            crate::tir::pipeline_cache::run_cached_tir_pipeline(
                &ir.functions,
                crate::tir::pipeline_cache::TirPipelineRunOptions {
                    target_info: native_tti.clone(),
                    cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Native,
                    cache: self.tir_cache.clone(),
                    process_externs: false,
                    tir_dump: env_setting(BackendEnv::TIR_DUMP).as_deref() == Some("1"),
                    tir_stats: env_setting(BackendEnv::TIR_OPT_STATS).as_deref() == Some("1"),
                    progress_prefix: Some("MOLT_BACKEND"),
                    resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
                },
                preprocess_backend_tir_input,
            )
            .cached_tir
        });
        mark("cached TIR pipeline", ir.functions.len());

        // Bound per-function optimization growth while transport allocations
        // can still be planned by the shared terminal ownership phase.
        if let (Some(target), Some(cached)) = (native_tti.as_ref(), native_cached_tir.as_mut()) {
            crate::tir::pipeline_cache::partition_cached_functions_before_drops(
                ir,
                cached,
                target,
                molt_tir::passes::max_function_ops(),
            );
        }
        mark("partition before drops", ir.functions.len());

        let mut final_functions = if use_llvm {
            None
        } else {
            let native_tti = native_tti.as_ref().expect("native target missing");
            let non_inlinable: HashSet<String> = if self.skip_shared_stdlib_partition {
                BTreeSet::new()
            } else {
                shared_stdlib_external_symbols(ir)
            }
            .into_iter()
            .collect();
            let signatures = molt_ir::linkage_abi::document_signatures(&ir.functions);
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
                    linkage: Some(&signatures),
                },
            );
            mark("module pipeline", ir.functions.len());
            // A specialized clone is another entry of its origin's row.
            typed.append_clone_rows(&mut ir.functions);
            let mut final_functions = NativeFinalFunctions::from_final_tir(
                SimpleIR {
                    functions: std::mem::take(&mut ir.functions),
                    profile: ir.profile.take(),
                },
                typed.functions,
            );
            if !self.skip_ir_passes {
                final_functions.retain_reachable(&module_registry_roots);
            }
            Some(final_functions)
        };
        if use_llvm && !self.skip_ir_passes {
            eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        }
        mark("final functions", ir.functions.len());
        if let Some(functions) = &final_functions {
            dump_rewritten_tir_if_requested(functions.bodies());
        }
        let emit_resolver_here = self.emit_app_callable_resolver;
        let app_callable_manifest =
            native_manifest.unwrap_or_else(|| self.take_app_callable_manifest(ir));
        if !self.skip_shared_stdlib_partition {
            if let Some(final_functions) = &mut final_functions {
                final_functions.externalize_partition(self.skip_ir_passes, &module_registry_roots);
            } else {
                externalize_shared_stdlib_partition(ir, &module_registry_roots);
            }
        }
        mark("manifest and partition", 0);
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }

        NativeProgramPipeline {
            emit_resolver_here,
            app_callable_manifest,
            final_functions,
        }
    }
}

fn detect_gpu_kernels(functions: &[FunctionIR]) {
    let mut gpu_kernel_names: Vec<String> = Vec::new();
    for func_ir in functions {
        let is_gpu = func_ir.ops.iter().any(|op| {
            matches!(
                op.kind,
                OpKind::GpuThreadId
                    | OpKind::GpuBlockId
                    | OpKind::GpuBlockDim
                    | OpKind::GpuGridDim
                    | OpKind::GpuBarrier
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
    let Some(pattern) = backend_env::var(BackendEnv::MOLT_DUMP_FUNC_IR).ok() else {
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
            dump.push_str(&format!(
                "{:4}: kind={:30} out={:20} var={:20} args={:40} val={:?} sval={:?}\n",
                idx,
                op.kind,
                op.out.as_deref().unwrap_or(""),
                op.var.as_deref().unwrap_or(""),
                op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                op.value,
                op.s_value
            ));
        }
        let _ = crate::debug_artifacts::write_debug_artifact(format!("ir/{sanitized}.txt"), dump);
    }
}

pub(super) fn dump_rewritten_tir_if_requested<'a>(
    mut bodies: impl Iterator<Item = &'a crate::tir::TirFunction>,
) {
    let Ok(requested) = backend_env::var(BackendEnv::MOLT_DUMP_REWRITTEN_FUNC) else {
        return;
    };
    if let Some(function) = bodies.find(|function| function.name == requested) {
        let _ = std::fs::write("tmp/rewritten_func_ir.txt", rewritten_ir_text(function));
    }
}

pub(super) fn rewritten_ir_text(function: &crate::tir::TirFunction) -> String {
    use molt_ir::tir::op_kinds_generated::opcode_canonical_kind_table;
    if function
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .any(|op| opcode_canonical_kind_table(op.opcode).is_none())
    {
        return crate::tir::printer::print_function(function);
    }
    let ops = crate::tir::lower_to_simple::lower_to_simple_ir(function);
    let mut dump = String::new();
    for (idx, op) in ops.iter().enumerate() {
        let _ = writeln!(dump, "{idx:04}: {:?}", op);
    }
    dump
}

pub(crate) fn preprocess_backend_tir_input(tmp_func: &mut FunctionIR) {
    if tmp_func.ops.iter().any(|op| op.kind == OpKind::Phi) {
        rewrite_phi_to_store_load(&mut tmp_func.ops);
        crate::tir::pipeline_cache::trace_tir_function_stage(
            &tmp_func.name,
            "after_phi_rewrite",
            tmp_func.ops.len(),
        );
    }
}
