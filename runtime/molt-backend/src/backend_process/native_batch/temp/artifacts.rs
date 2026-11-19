use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use molt_ir::backend_environment::BackendEnv;
use molt_ir::runtime_callable_symbols::read_selected_callable_input;

use super::super::super::io_limits::write_json_artifact;
use super::super::batching::NativeBatchObjectJob;
use super::sanitize::sanitize_debug_artifact_component;

pub(crate) fn preserve_native_batch_worker_failure_artifacts(
    label: &str,
    job_bytes: &[u8],
    job_name: &str,
    object_path: &Path,
) -> io::Result<PathBuf> {
    let job: NativeBatchObjectJob = serde_json::from_slice(job_bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid failed native batch object job '{job_name}': {err}"),
        )
    })?;
    let job_stem = Path::new(job_name)
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

    let copied_job = artifact_dir.join(format!("{job_stem}.json"));
    molt_artifact_publish::write_bytes_atomically(&copied_job, job_bytes)?;

    let callable_input = preserve_runtime_callable_input(&job, &artifact_dir)?;
    let copied_object = preserve_partial_object(object_path, &artifact_dir)?;
    let replay_object = artifact_dir.join("replay.o");
    // Replay runs the executable that spawned the failed worker, with the
    // worker's own arguments (worker/spawn.rs); a fixed build path names a
    // binary that release and daemon builds never had.
    let executable = std::env::current_exe().map_err(|err| {
        io::Error::other(format!(
            "failed to resolve the backend executable for the replay command: {err}"
        ))
    })?;
    let mut argv = vec![
        executable.display().to_string(),
        "--native-batch-job-file".to_string(),
        copied_job.display().to_string(),
        "--output".to_string(),
        replay_object.display().to_string(),
    ];
    if let Some(scope) = crate::backend_process::origin::current_scope() {
        argv.push("--emission-scope".to_string());
        argv.push(scope.to_string());
    }
    // The worker inherited the callable input's path, a transport setting
    // outside the job; replay reads the preserved copy, or none.
    let mut environment = serde_json::to_value(&job.codegen_environment.values)?;
    environment
        .as_object_mut()
        .ok_or_else(|| io::Error::other("codegen environment is not a JSON object"))?
        .insert(
            BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS.name().to_string(),
            callable_input
                .as_ref()
                .map(|(path, _)| path.display().to_string())
                .into(),
        );
    let manifest = serde_json::json!({
        "schema_version": 3,
        "label": label,
        "job_name": job_name,
        "source_object_path": object_path.display().to_string(),
        "copied_job_path": copied_job.display().to_string(),
        "copied_object_path": copied_object.as_ref().map(|path| path.display().to_string()),
        "module_context": "embedded-in-job",
        "runtime_callable_input": callable_input.as_ref().map(|(path, sha256)| serde_json::json!({
            "path": path.display().to_string(),
            "sha256": sha256,
        })),
        "replay": {
            "environment": environment,
            "environment_null_means": "unset before replay",
            "argv": argv
        }
    });
    write_json_artifact(&manifest_marker, &manifest)?;
    Ok(artifact_dir)
}

/// Copy the exact runtime callable input the failed worker admitted, verified
/// against the digest its job records. The worker read it from a path the
/// build removes; without the copy a replay fails admission before codegen.
fn preserve_runtime_callable_input(
    job: &NativeBatchObjectJob,
    artifact_dir: &Path,
) -> io::Result<Option<(PathBuf, String)>> {
    let recorded = job
        .codegen_environment
        .values
        .get(&BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256)
        .cloned()
        .flatten();
    let selected = read_selected_callable_input().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to preserve the admitted runtime callable input: {err}"),
        )
    })?;
    match (recorded, selected) {
        (None, None) => Ok(None),
        (Some(recorded), Some(input)) if recorded == input.sha256 => {
            let path = artifact_dir.join("runtime_callable_symbols.txt");
            molt_artifact_publish::write_bytes_atomically(&path, &input.bytes)?;
            Ok(Some((path, input.sha256)))
        }
        (recorded, selected) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the failed job admitted runtime callable input {recorded:?}, but the request now selects {:?}",
                selected.map(|input| input.sha256)
            ),
        )),
    }
}

fn preserve_partial_object(object_path: &Path, artifact_dir: &Path) -> io::Result<Option<PathBuf>> {
    if !object_path.exists() {
        return Ok(None);
    }
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
    Ok(Some(copied_object))
}
