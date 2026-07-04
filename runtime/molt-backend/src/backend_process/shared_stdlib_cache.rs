use super::*;

#[cfg(feature = "native-backend")]
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

#[cfg(feature = "native-backend")]
pub(crate) struct NativeStdlibCachePrepare<'a> {
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_obj_path: Option<&'a str>,
    pub(crate) expected_cache_key: Option<&'a str>,
    pub(crate) expected_cache_manifest: Option<&'a str>,
    pub(crate) have_entry_module: bool,
    pub(crate) entry_module: &'a str,
    pub(crate) explicit_stdlib_module_symbols: Option<&'a std::collections::BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock).  Its init symbols root the
    /// stdlib-partition dead-function elimination and it is forwarded to the
    /// application-object compile for blob emission.
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
}

#[cfg(feature = "native-backend")]
pub(crate) fn prepare_native_application_object<'a>(
    ir: &mut SimpleIR,
    request: NativeStdlibCachePrepare<'a>,
) -> io::Result<NativeApplicationObjectOptions<'a>> {
    let app_callable_manifest = request
        .stdlib_obj_path
        .map(|_| molt_backend::compute_app_callable_manifest_checked(&ir.functions));
    let module_registry_roots: std::collections::BTreeSet<String> = request
        .module_registry
        .as_ref()
        .map(|registry| registry.init_symbols.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(stdlib_path_str) = request.stdlib_obj_path {
        let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
            ir,
            request.entry_module,
            request.explicit_stdlib_module_symbols,
            &module_registry_roots,
        );
        let stdlib_path = Path::new(stdlib_path_str);
        ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(|err| {
            eprintln!(
                "{}: warning: failed to create stdlib parent: {err}",
                request.log_prefix
            );
        });

        let current_partition_manifest =
            shared_stdlib_partition_manifest(&stdlib_funcs).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to compute shared stdlib partition manifest: {err}"),
                )
            })?;
        let split_function_names =
            shared_stdlib_split_function_names(&user_remaining, &stdlib_funcs);
        if let Err(err) = validate_shared_stdlib_partition(&stdlib_funcs, &split_function_names) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(io::Error::new(
                err.kind(),
                format!("invalid shared stdlib partition: {err}"),
            ));
        }

        if request.have_entry_module && stdlib_path.exists() {
            let current_stdlib_count = stdlib_funcs.len();
            let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
            let cached_count: usize = std::fs::read_to_string(&count_path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            if shared_stdlib_cache_matches(
                stdlib_path,
                request.expected_cache_key,
                request.expected_cache_manifest,
                Some(current_partition_manifest.as_str()),
            ) {
                let mut retained = std::mem::take(&mut user_remaining);
                let mut extern_count = 0usize;
                for mut func in std::mem::take(&mut stdlib_funcs) {
                    molt_backend::externalize_function_with_signature(&mut func);
                    extern_count += 1;
                    retained.push(func);
                }
                let user_count = retained.len().saturating_sub(extern_count);
                ir.functions = retained;
                eprintln!(
                    "{}: incremental -- compiling {user_count} user functions \
                     ({extern_count} stdlib extern from {})",
                    request.log_prefix,
                    stdlib_path.display()
                );
            } else {
                let cached_key = read_stdlib_cache_key(stdlib_path);
                let cached_manifest = read_stdlib_cache_manifest(stdlib_path);
                let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
                eprintln!(
                    "{}: stdlib cache contract mismatch \
                     (cached key {}, expected key {}; cached manifest {}, expected manifest present {}; \
                     cached partition manifest present {}, expected partition manifest present true; \
                     cached {} functions, need {}) -- rebuilding",
                    request.log_prefix,
                    cached_key.as_deref().unwrap_or("<missing>"),
                    request.expected_cache_key.unwrap_or("<missing>"),
                    cached_manifest.as_deref().unwrap_or("<missing>"),
                    request.expected_cache_manifest.is_some(),
                    cached_partition_manifest.is_some(),
                    cached_count,
                    current_stdlib_count,
                );
                remove_shared_stdlib_cache_artifacts(stdlib_path);
            }
        }

        if !stdlib_path.exists() {
            ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(|err| {
                eprintln!(
                    "{}: warning: could not create stdlib cache parent dir: {err}",
                    request.log_prefix
                );
            });

            let stdlib_count = stdlib_funcs.len();
            eprintln!(
                "{}: first build -- caching {} stdlib functions to {}",
                request.log_prefix,
                stdlib_count,
                stdlib_path.display()
            );
            let temp_stdlib_path = stdlib_cache_temp_publish_path(stdlib_path, "object");
            if let Err(err) = compile_stdlib_cache_object(
                &temp_stdlib_path,
                std::mem::take(&mut stdlib_funcs),
                ir.profile.clone(),
                request.target_triple,
                request.log_prefix,
            ) {
                let _ = std::fs::remove_file(&temp_stdlib_path);
                return Err(io::Error::new(
                    err.kind(),
                    format!("failed to materialize shared stdlib cache: {err}"),
                ));
            }
            if let Err(err) = publish_shared_stdlib_cache_object(
                stdlib_path,
                &temp_stdlib_path,
                stdlib_count,
                request.expected_cache_key,
                request.expected_cache_manifest,
                current_partition_manifest.as_str(),
            ) {
                let _ = std::fs::remove_file(&temp_stdlib_path);
                return Err(io::Error::new(
                    err.kind(),
                    format!("failed to publish shared stdlib cache: {err}"),
                ));
            }

            ir.functions = std::mem::take(&mut user_remaining);
            eprintln!(
                "{}: compiling {} user functions",
                request.log_prefix,
                ir.functions.len()
            );
        }
    }

    Ok(NativeApplicationObjectOptions {
        target_triple: request.target_triple,
        stdlib_split_enabled: request.stdlib_obj_path.is_some(),
        app_callable_manifest,
        log_prefix: request.log_prefix,
        module_registry: request.module_registry,
    })
}
