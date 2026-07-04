use super::*;

pub(crate) fn compile_stdlib_cache_object(
    stdlib_path: &Path,
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
) -> io::Result<()> {
    let stdlib_count = stdlib_funcs.len();
    if stdlib_count == 0 {
        eprintln!("{log_prefix}: stdlib cache is empty (0 reachable functions)");
        let stdlib_ir = SimpleIR {
            functions: Vec::new(),
            profile,
        };
        let mut stdlib_backend = SimpleBackend::new_with_target(target_triple);
        stdlib_backend.skip_ir_passes = true;
        stdlib_backend.skip_shared_stdlib_partition = true;
        stdlib_backend.emit_app_callable_resolver = false;
        let stdlib_output = stdlib_backend.compile(stdlib_ir);
        std::fs::write(stdlib_path, &stdlib_output.bytes)?;
        return Ok(());
    }

    let stdlib_batch_size = resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE);
    let stdlib_batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    let all_stdlib_names: std::collections::BTreeSet<String> =
        stdlib_funcs.iter().map(|f| f.name.clone()).collect();
    let stdlib_module_context = SimpleBackend::build_module_context(&stdlib_funcs);
    let stdlib_batches =
        partition_functions_for_batches(stdlib_funcs, stdlib_batch_size, stdlib_batch_ops_budget);
    let stdlib_total_batches = stdlib_batches.len();

    if stdlib_total_batches == 1 {
        let batch_funcs = stdlib_batches.into_iter().next().unwrap_or_default();
        let batch_ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
        eprintln!(
            "{log_prefix}: stdlib batch 1/1 ({} functions, {} ops / budget {})",
            batch_funcs.len(),
            batch_ops,
            stdlib_batch_ops_budget
        );
        let stdlib_ir = SimpleIR {
            functions: batch_funcs,
            profile,
        };
        let mut stdlib_backend = SimpleBackend::new_with_target(target_triple);
        stdlib_backend.skip_ir_passes = true;
        stdlib_backend.skip_shared_stdlib_partition = true;
        // The stdlib cache object is not the main application object; the per-app
        // resolver is emitted once, into the main object.
        stdlib_backend.emit_app_callable_resolver = false;
        let stdlib_output = stdlib_backend.compile(stdlib_ir);
        std::fs::write(stdlib_path, &stdlib_output.bytes)?;
        return Ok(());
    }

    let stdlib_tmp_dir =
        std::env::temp_dir().join(format!("molt_stdlib_batch_{}", std::process::id()));
    std::fs::create_dir_all(&stdlib_tmp_dir)?;
    let module_context_path = stdlib_tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata {
            module_context: stdlib_module_context,
        },
    )?;
    let mut stdlib_batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        for (stdlib_batch_idx, batch_funcs) in stdlib_batches.into_iter().enumerate() {
            let batch_ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
            eprintln!(
                "{log_prefix}: stdlib batch {}/{} ({} functions, {} ops / budget {})",
                stdlib_batch_idx + 1,
                stdlib_total_batches,
                batch_funcs.len(),
                batch_ops,
                stdlib_batch_ops_budget
            );
            let batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&all_stdlib_names, &batch_ir.functions);
            let job_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.json"));
            let batch_path = stdlib_tmp_dir.join(format!("batch_{stdlib_batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: target_triple.map(str::to_owned),
                    emit_app_callable_resolver: false,
                    app_callable_manifest: None,
                    external_function_names,
                    // Stdlib cache objects never own the registry blob; the
                    // main application object emits it.
                    module_registry: None,
                },
            )?;
            stdlib_batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut stdlib_batch_paths: Vec<std::path::PathBuf> =
            Vec::with_capacity(stdlib_batch_specs.len());
        for (stdlib_batch_idx, spec) in stdlib_batch_specs.iter().enumerate() {
            eprintln!(
                "{log_prefix}: compiling materialized stdlib batch {}/{}",
                stdlib_batch_idx + 1,
                stdlib_total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native stdlib batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            stdlib_batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(stdlib_path, &stdlib_batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &stdlib_tmp_dir,
        "native stdlib batch temp dir",
        compile_result,
    )
}
