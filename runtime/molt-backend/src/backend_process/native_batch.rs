use super::*;

pub(crate) fn partition_functions_for_batches(
    functions: Vec<molt_backend::FunctionIR>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
) -> Vec<Vec<molt_backend::FunctionIR>> {
    let max_functions_per_batch = max_functions_per_batch.max(1);
    let max_ops_per_batch = max_ops_per_batch.max(1);

    let mut batches: Vec<Vec<molt_backend::FunctionIR>> = Vec::new();
    let mut current: Vec<molt_backend::FunctionIR> = Vec::new();
    let mut current_ops = 0usize;

    for func in functions {
        let func_ops = func.ops.len();
        let would_overflow_count = current.len() >= max_functions_per_batch;
        let would_overflow_ops =
            !current.is_empty() && current_ops.saturating_add(func_ops) > max_ops_per_batch;

        if would_overflow_count || would_overflow_ops {
            batches.push(std::mem::take(&mut current));
            current_ops = 0;
        }

        current_ops = current_ops.saturating_add(func_ops);
        current.push(func);
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

pub(crate) fn batch_external_function_names(
    all_function_names: &std::collections::BTreeSet<String>,
    batch_funcs: &[molt_backend::FunctionIR],
) -> std::collections::BTreeSet<String> {
    let batch_names: std::collections::BTreeSet<&str> =
        batch_funcs.iter().map(|func| func.name.as_str()).collect();
    all_function_names
        .iter()
        .filter(|name| !batch_names.contains(name.as_str()))
        .cloned()
        .collect()
}

#[cfg(all(feature = "native-backend", target_os = "macos"))]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe extern "C" {
        fn malloc_default_zone() -> *mut libc::c_void;
        fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
    }

    unsafe {
        let zone = malloc_default_zone();
        if !zone.is_null() {
            let _ = malloc_zone_pressure_relief(zone, usize::MAX);
        }
    }
}

#[cfg(all(feature = "native-backend", target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_native_backend_batch_memory_to_os() {
    unsafe {
        let _ = libc::malloc_trim(0);
    }
}

#[cfg(all(
    feature = "native-backend",
    not(target_os = "macos"),
    not(all(target_os = "linux", target_env = "gnu"))
))]
pub(crate) fn release_native_backend_batch_memory_to_os() {}

pub(crate) fn resolved_batch_size_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

pub(crate) fn resolved_batch_op_budget_limit(default: usize) -> usize {
    let raw = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}

#[cfg(feature = "native-backend")]
pub(crate) struct NativeApplicationObjectOptions<'a> {
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_split_enabled: bool,
    pub(crate) app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeApplicationObjectResult {
    pub(crate) function_count: usize,
    pub(crate) batch_count: usize,
}

#[cfg(feature = "native-backend")]
#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchModuleMetadata {
    pub(crate) module_context: molt_backend::NativeBackendModuleContext,
}

#[cfg(feature = "native-backend")]
#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchObjectJob {
    pub(crate) ir: SimpleIR,
    pub(crate) module_context_path: PathBuf,
    pub(crate) target_triple: Option<String>,
    pub(crate) emit_app_intrinsic_resolver: bool,
    pub(crate) app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    pub(crate) external_function_names: std::collections::BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone)]
pub(crate) struct NativeBatchJobSpec {
    pub(crate) job_path: PathBuf,
    pub(crate) object_path: PathBuf,
}

#[cfg(feature = "native-backend")]
pub(crate) fn deduplicate_functions_by_name(functions: &mut Vec<molt_backend::FunctionIR>) {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    functions.retain(|f| seen.insert(f.name.clone()));
}

pub(crate) fn relocatable_linker_binary(linker_override: Option<&str>) -> String {
    linker_override
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("MOLT_LINKER").ok())
        .or_else(|| std::env::var("LD").ok())
        .or_else(|| std::env::var("CC").ok())
        .unwrap_or_else(|| "ld".to_string())
}

pub(crate) fn merge_relocatable_objects(
    output_path: &Path,
    object_paths: &[std::path::PathBuf],
    linker_override: Option<&str>,
) -> io::Result<()> {
    if object_paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no object files to merge",
        ));
    }

    ensure_output_parent_dir(output_path.to_str().unwrap_or_default())?;

    if object_paths.len() == 1 {
        std::fs::copy(&object_paths[0], output_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to copy batch object '{}' to '{}': {}",
                    object_paths[0].display(),
                    output_path.display(),
                    err
                ),
            )
        })?;
        return Ok(());
    }

    let ld_bin = relocatable_linker_binary(linker_override);
    let mut cmd = std::process::Command::new(&ld_bin);
    if ld_bin.contains("clang") || ld_bin.contains("gcc") {
        cmd.arg("-Wl,-r").arg("-o").arg(output_path);
    } else {
        cmd.arg("-r").arg("-o").arg(output_path);
    }
    for path in object_paths {
        cmd.arg(path);
    }
    let merge_output = cmd.output().map_err(|err| {
        io::Error::other(format!(
            "failed to run relocatable linker '{ld_bin}' for '{}': {err}",
            output_path.display()
        ))
    })?;
    if merge_output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&merge_output.stderr)
        .trim()
        .to_string();
    let detail = if stderr.is_empty() {
        format!("exit {}", merge_output.status)
    } else {
        stderr
    };
    Err(io::Error::other(format!(
        "relocatable link failed via '{ld_bin}' for '{}': {detail}",
        output_path.display()
    )))
}

#[cfg(feature = "native-backend")]
pub(crate) fn remove_native_batch_temp_dir(path: &Path, label: &str) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("failed to remove {label} '{}': {err}", path.display()),
        )),
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn finish_native_batch_temp_dir(
    path: &Path,
    label: &str,
    compile_result: io::Result<()>,
) -> io::Result<()> {
    let cleanup_result = remove_native_batch_temp_dir(path, label);
    match (compile_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(compile_err), Ok(())) => Err(compile_err),
        (Err(compile_err), Err(cleanup_err)) => {
            eprintln!(
                "MOLT_BACKEND: failed to clean {label} '{}' after compile error: {cleanup_err}",
                path.display()
            );
            Err(compile_err)
        }
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn compile_native_batch_object_job(
    job: NativeBatchObjectJob,
    output_path: &Path,
) -> io::Result<()> {
    let metadata: NativeBatchModuleMetadata =
        read_json_artifact(&job.module_context_path, "native batch module metadata")?;
    let mut backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
    backend.skip_ir_passes = true;
    backend.skip_shared_stdlib_partition = true;
    backend.emit_app_intrinsic_resolver = job.emit_app_intrinsic_resolver;
    backend.app_intrinsic_manifest = job.app_intrinsic_manifest;
    backend.external_function_names = job.external_function_names;
    backend.set_module_context(metadata.module_context);
    let output = backend.compile(job.ir);
    write_output_path(output_path, &output.bytes)
}

#[cfg(feature = "native-backend")]
pub(crate) fn compile_native_batch_object_job_file(
    job_path: &Path,
    output_path: &Path,
) -> io::Result<()> {
    let job: NativeBatchObjectJob = read_json_artifact(job_path, "native batch object job")?;
    compile_native_batch_object_job(job, output_path)
}

#[cfg(feature = "native-backend")]
pub(crate) fn sanitize_debug_artifact_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "artifact".to_string()
    } else {
        sanitized.to_string()
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn preserve_native_batch_worker_failure_artifacts(
    label: &str,
    job_path: &Path,
    object_path: &Path,
) -> io::Result<PathBuf> {
    let mut job: NativeBatchObjectJob =
        read_json_artifact(job_path, "failed native batch object job")?;
    let job_stem = job_path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(sanitize_debug_artifact_component)
        .unwrap_or_else(|| "batch".to_string());
    let label_component = sanitize_debug_artifact_component(label);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let manifest_marker = molt_backend::debug_artifacts::prepare_debug_artifact_path(format!(
        "native-batch-failures/{label_component}/{}-{nonce}-{job_stem}/manifest.json",
        std::process::id()
    ))?;
    let artifact_dir = manifest_marker
        .parent()
        .ok_or_else(|| io::Error::other("debug artifact path has no parent"))?
        .to_path_buf();
    std::fs::create_dir_all(&artifact_dir)?;

    let copied_module_context = artifact_dir.join("module_context.json");
    std::fs::copy(&job.module_context_path, &copied_module_context).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to preserve native batch module context '{}' to '{}': {err}",
                job.module_context_path.display(),
                copied_module_context.display()
            ),
        )
    })?;
    let original_module_context_path = job.module_context_path.clone();
    job.module_context_path = copied_module_context.clone();

    let copied_job = artifact_dir.join(
        job_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("batch.json")),
    );
    write_json_artifact(&copied_job, &job)?;

    let copied_object = if object_path.exists() {
        let copied_object = artifact_dir.join(
            object_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("batch.o")),
        );
        std::fs::copy(object_path, &copied_object).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to preserve partial native batch object '{}' to '{}': {err}",
                    object_path.display(),
                    copied_object.display()
                ),
            )
        })?;
        Some(copied_object)
    } else {
        None
    };
    let replay_object = artifact_dir.join("replay.o");
    let manifest = serde_json::json!({
        "schema_version": 1,
        "label": label,
        "source_job_path": job_path.display().to_string(),
        "source_object_path": object_path.display().to_string(),
        "source_module_context_path": original_module_context_path.display().to_string(),
        "copied_job_path": copied_job.display().to_string(),
        "copied_object_path": copied_object.as_ref().map(|path| path.display().to_string()),
        "copied_module_context_path": copied_module_context.display().to_string(),
        "replay": {
            "argv": [
                "target/debug/molt-backend",
                "--native-batch-job-file",
                copied_job.display().to_string(),
                "--output",
                replay_object.display().to_string()
            ]
        }
    });
    write_json_artifact(&manifest_marker, &manifest)?;
    Ok(artifact_dir)
}

#[cfg(feature = "native-backend")]
pub(crate) fn run_native_batch_worker_with_failure_artifacts(
    label: &str,
    job_path: &Path,
    object_path: &Path,
) -> io::Result<()> {
    match run_native_batch_worker(job_path, object_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let original_error = err.to_string();
            match preserve_native_batch_worker_failure_artifacts(label, job_path, object_path) {
                Ok(artifact_dir) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; preserved replayable {label} artifacts at '{}'",
                        artifact_dir.display()
                    ),
                )),
                Err(preserve_err) => Err(io::Error::new(
                    err.kind(),
                    format!(
                        "{original_error}; additionally failed to preserve {label} artifacts: {preserve_err}"
                    ),
                )),
            }
        }
    }
}

#[cfg(all(feature = "native-backend", not(test)))]
pub(crate) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    let exe = std::env::current_exe().map_err(|err| {
        io::Error::other(format!(
            "failed to resolve current backend executable for batch worker: {err}"
        ))
    })?;
    let status = std::process::Command::new(&exe)
        .arg("--native-batch-job-file")
        .arg(job_path)
        .arg("--output")
        .arg(object_path)
        .status()
        .map_err(|err| {
            io::Error::other(format!(
                "failed to spawn native batch worker '{}' for '{}': {err}",
                exe.display(),
                job_path.display()
            ))
        })?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "native batch worker failed for '{}' with {status}",
        job_path.display()
    )))
}

#[cfg(all(feature = "native-backend", test))]
pub(crate) fn run_native_batch_worker(job_path: &Path, object_path: &Path) -> io::Result<()> {
    compile_native_batch_object_job_file(job_path, object_path)
}

#[cfg(feature = "native-backend")]
pub(crate) fn compile_native_application_object_to_path(
    mut ir: SimpleIR,
    output_path: &Path,
    mut options: NativeApplicationObjectOptions<'_>,
) -> io::Result<NativeApplicationObjectResult> {
    if options.stdlib_split_enabled && options.app_intrinsic_manifest.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stdlib-split native application object requires a full-set app intrinsic manifest",
        ));
    }

    // Preserve the one-shot native application-object sequence as the single
    // authority for both direct backend runs and daemon requests.
    molt_backend::inject_runtime_exit(&mut ir);
    if !options.stdlib_split_enabled {
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
    }
    deduplicate_functions_by_name(&mut ir.functions);

    let function_count = ir.functions.len();
    let batch_size = resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE);
    let batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    let total_ops = ir
        .functions
        .iter()
        .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
    if function_count <= batch_size && total_ops <= batch_ops_budget {
        let mut backend = SimpleBackend::new_with_target(options.target_triple);
        if options.stdlib_split_enabled {
            backend.skip_shared_stdlib_partition = true;
        }
        backend.app_intrinsic_manifest = options.app_intrinsic_manifest.take();
        let obj_output = backend.compile(ir);
        write_output_path(output_path, &obj_output.bytes)?;
        eprintln!(
            "Successfully compiled to {} ({} functions)",
            output_path.display(),
            function_count
        );
        return Ok(NativeApplicationObjectResult {
            function_count,
            batch_count: 1,
        });
    }

    let profile = ir.profile;
    let all_functions: Vec<_> = ir.functions.into_iter().collect();
    let all_func_names: std::collections::BTreeSet<String> =
        all_functions.iter().map(|f| f.name.clone()).collect();
    let module_context = SimpleBackend::build_module_context(&all_functions);
    if options.app_intrinsic_manifest.is_none() {
        options.app_intrinsic_manifest = Some(molt_backend::compute_intrinsic_manifest_checked(
            &all_functions,
        ));
    }
    let batches = partition_functions_for_batches(all_functions, batch_size, batch_ops_budget);
    let total_batches = batches.len();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt_batch_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir)?;
    let module_context_path = tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata { module_context },
    )?;
    let mut batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        for (batch_idx, batch_funcs) in batches.into_iter().enumerate() {
            let batch_ops = batch_funcs
                .iter()
                .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
            eprintln!(
                "{}: batch {}/{total_batches} ({} functions, {} ops / budget {})",
                options.log_prefix,
                batch_idx + 1,
                batch_funcs.len(),
                batch_ops,
                batch_ops_budget
            );
            let batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&all_func_names, &batch_ir.functions);
            let job_path = tmp_dir.join(format!("batch_{batch_idx}.json"));
            let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: options.target_triple.map(str::to_owned),
                    emit_app_intrinsic_resolver: batch_idx == 0,
                    app_intrinsic_manifest: if batch_idx == 0 {
                        options.app_intrinsic_manifest.take()
                    } else {
                        None
                    },
                    external_function_names,
                },
            )?;
            batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut batch_paths: Vec<std::path::PathBuf> = Vec::with_capacity(batch_specs.len());
        for (batch_idx, spec) in batch_specs.iter().enumerate() {
            eprintln!(
                "{}: compiling materialized batch {}/{}",
                options.log_prefix,
                batch_idx + 1,
                total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native application batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(output_path, &batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &tmp_dir,
        "native application batch temp dir",
        compile_result,
    )?;

    eprintln!(
        "Successfully compiled to {} ({} functions, {} batches)",
        output_path.display(),
        function_count,
        total_batches
    );
    Ok(NativeApplicationObjectResult {
        function_count,
        batch_count: total_batches,
    })
}
