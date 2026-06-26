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
        stdlib_backend.emit_app_intrinsic_resolver = false;
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
        stdlib_backend.emit_app_intrinsic_resolver = false;
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
                    emit_app_intrinsic_resolver: false,
                    app_intrinsic_manifest: None,
                    external_function_names,
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
pub(crate) fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
pub(crate) fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
pub(crate) fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let entry_init = format!("molt_init_{entry_module}");
    if name == "molt_main"
        || name == "molt_host_init"
        || name.starts_with(&format!("{entry_module}__"))
        || name == entry_init
        || name == "molt_init___main__"
        || name == "molt_isolate_import"
        || name == "molt_isolate_bootstrap"
    {
        return true;
    }
    if let Some(stdlib_module_symbols) = stdlib_module_symbols {
        if let Some(module_symbol) = emitted_module_symbol(name) {
            return !stdlib_module_symbols.contains(module_symbol);
        }
        return !stdlib_module_symbols
            .iter()
            .any(|module_symbol| emitted_name_matches_module_symbol(name, module_symbol));
    }
    false
}

#[cfg(feature = "native-backend")]
pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
) -> (Vec<molt_backend::FunctionIR>, Vec<molt_backend::FunctionIR>) {
    molt_backend::inject_runtime_exit(ir);
    molt_backend::eliminate_dead_functions(ir);
    molt_backend::eliminate_dead_imports(ir);
    molt_backend::eliminate_dead_ops(ir);
    let user_func_set: std::collections::BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| is_user_owned_symbol(&f.name, entry_module, stdlib_module_symbols))
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs)
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("count")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("key")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("manifest.json")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_partition_manifest_sidecar_path(
    stdlib_path: &Path,
) -> std::path::PathBuf {
    stdlib_path.with_extension("partition.json")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_object_digest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("sha256")
}

#[cfg(feature = "native-backend")]
pub(crate) fn sha256_file_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = hasher.finalize();
    let bytes: &[u8] = digest.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

#[cfg(feature = "native-backend")]
pub(crate) const STDLIB_PARTITION_MANIFEST_SCHEMA: &str = "stdlib-partition-v1";

#[cfg(feature = "native-backend")]
pub(crate) fn update_fnv1a64(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_partition_manifest(
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> io::Result<String> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut funcs: Vec<&molt_backend::FunctionIR> = stdlib_funcs.iter().collect();
    funcs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut body_hash = FNV_OFFSET;
    for func in funcs {
        names.push(func.name.clone());
        body_hash = update_fnv1a64(body_hash, func.name.as_bytes());
        body_hash = update_fnv1a64(body_hash, &[0]);
        let body = serde_json::to_vec(&serde_json::json!({
            "name": &func.name,
            "params": &func.params,
            "ops": &func.ops,
            "param_types": &func.param_types,
            "source_file": &func.source_file,
            "is_extern": func.is_extern,
        }))
        .map_err(io::Error::other)?;
        body_hash = update_fnv1a64(body_hash, &body);
        body_hash = update_fnv1a64(body_hash, &[0xff]);
    }

    serde_json::to_string(&serde_json::json!({
        "schema": STDLIB_PARTITION_MANIFEST_SCHEMA,
        "function_count": names.len(),
        "functions": names,
        "body_hash": format!("{body_hash:016x}"),
    }))
    .map_err(io::Error::other)
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_partition_reference_kind(kind: &str) -> bool {
    matches!(
        kind,
        "call"
            | "call_internal"
            | "func_new"
            | "func_new_closure"
            | "func_new_builtin"
            | "code_new"
            | "call_guarded"
            | "call_indirect"
            | "alloc_task"
            | "generator_create"
            | "coro_create"
            | "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "task_new"
            | "generator_send"
            | "spawn"
            | "call_func"
            | "call_method"
            | "import_from"
            | "import_name"
            | "class_def"
            | "decorator"
            | "super_call"
            | "yield_from"
            | "await"
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_partition_closure_issue(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let partition_names: std::collections::BTreeSet<&str> =
        stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
    let mut missing: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for func in stdlib_funcs {
        for op in &func.ops {
            if !stdlib_partition_reference_kind(op.kind.as_str()) {
                continue;
            }
            let Some(target) = op.s_value.as_deref() else {
                continue;
            };
            if !all_function_names.contains(target) {
                continue;
            }
            if !partition_names.contains(target) {
                missing.insert(format!("{} -> {}", func.name, target));
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    let preview: Vec<_> = missing.iter().take(8).cloned().collect();
    let suffix = if missing.len() > preview.len() {
        ", ..."
    } else {
        ""
    };
    Some(format!(
        "shared stdlib partition has unresolved SimpleIR function references: {}{}",
        preview.join(", "),
        suffix
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn validate_shared_stdlib_partition(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> io::Result<()> {
    if let Some(issue) = shared_stdlib_partition_closure_issue(stdlib_funcs, all_function_names) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, issue));
    }
    Ok(())
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_split_function_names(
    user_funcs: &[molt_backend::FunctionIR],
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> std::collections::BTreeSet<String> {
    user_funcs
        .iter()
        .chain(stdlib_funcs.iter())
        .map(|func| func.name.clone())
        .collect()
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_publish_lock_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_file_name(format!(
        "{}.publish.lock",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared")
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_temp_publish_path(stdlib_path: &Path, label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    stdlib_path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared"),
        std::process::id(),
        stamp,
        label,
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn atomic_replace_file(temp_path: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if final_path.exists() {
        let _ = std::fs::remove_file(final_path);
    }
    std::fs::rename(temp_path, final_path)
}

#[cfg(feature = "native-backend")]
pub(crate) fn sync_published_file(path: &Path) -> io::Result<()> {
    File::options()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

#[cfg(feature = "native-backend")]
pub(crate) fn write_atomic_text_file(path: &Path, contents: &str) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let temp_path = stdlib_cache_temp_publish_path(path, "text");
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    sync_published_file(path)?;
    Ok(())
}

#[cfg(all(feature = "native-backend", unix))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let lock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if lock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if unlock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", windows))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let mut overlapped = OVERLAPPED::default();
    let lock_rc = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if lock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    if unlock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", not(any(unix, windows))))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    _stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    body()
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_key_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) {
    let _ = std::fs::remove_file(stdlib_path);
    let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_object_digest_sidecar_path(stdlib_path));
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_cache_matches(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    let Some(expected_key) = expected_key.filter(|key| !key.is_empty()) else {
        return false;
    };
    let Some(expected_manifest) = expected_manifest.filter(|manifest| !manifest.is_empty()) else {
        return false;
    };
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return false;
    }
    let Ok(actual_object_digest) = sha256_file_hex(stdlib_path) else {
        return false;
    };
    let Ok(cached_object_digest) =
        std::fs::read_to_string(stdlib_cache_object_digest_sidecar_path(stdlib_path))
    else {
        return false;
    };
    if cached_object_digest.trim() != actual_object_digest {
        return false;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
    {
        return cached_partition_manifest.as_deref() == Some(expected_partition_manifest);
    }
    cached_partition_manifest.is_some()
}

#[cfg(feature = "native-backend")]
pub(crate) fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
    write_atomic_text_file(&count_path, &stdlib_count.to_string())?;

    let key_path = stdlib_cache_key_sidecar_path(stdlib_path);
    if let Some(cache_key) = cache_key.filter(|key| !key.is_empty()) {
        write_atomic_text_file(&key_path, cache_key)?;
    } else {
        match std::fs::remove_file(&key_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    let manifest_path = stdlib_cache_manifest_sidecar_path(stdlib_path);
    if let Some(cache_manifest) = cache_manifest.filter(|manifest| !manifest.is_empty()) {
        write_atomic_text_file(&manifest_path, cache_manifest)?;
    } else {
        match std::fs::remove_file(&manifest_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    write_atomic_text_file(
        &stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        partition_manifest,
    )?;
    let object_digest = sha256_file_hex(stdlib_path)?;
    write_atomic_text_file(
        &stdlib_cache_object_digest_sidecar_path(stdlib_path),
        &object_digest,
    )?;
    Ok(())
}

#[cfg(feature = "native-backend")]
pub(crate) fn publish_shared_stdlib_cache_object(
    stdlib_path: &Path,
    temp_object_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let result = with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        if let Err(err) = atomic_replace_file(temp_object_path, stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = sync_published_file(stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = write_shared_stdlib_cache_sidecars(
            stdlib_path,
            stdlib_count,
            cache_key,
            cache_manifest,
            partition_manifest,
        ) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temp_object_path);
    }
    result
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
}

#[cfg(feature = "native-backend")]
pub(crate) fn prepare_native_application_object<'a>(
    ir: &mut SimpleIR,
    request: NativeStdlibCachePrepare<'a>,
) -> io::Result<NativeApplicationObjectOptions<'a>> {
    let app_intrinsic_manifest = request
        .stdlib_obj_path
        .map(|_| molt_backend::compute_intrinsic_manifest_checked(&ir.functions));

    if let Some(stdlib_path_str) = request.stdlib_obj_path {
        let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
            ir,
            request.entry_module,
            request.explicit_stdlib_module_symbols,
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
        app_intrinsic_manifest,
        log_prefix: request.log_prefix,
    })
}
