use super::*;

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
