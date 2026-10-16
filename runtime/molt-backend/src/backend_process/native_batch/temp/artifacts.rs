use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

    let copied_object = preserve_partial_object(object_path, &artifact_dir)?;
    let replay_object = artifact_dir.join("replay.o");
    let manifest = serde_json::json!({
        "schema_version": 2,
        "label": label,
        "job_name": job_name,
        "source_object_path": object_path.display().to_string(),
        "copied_job_path": copied_job.display().to_string(),
        "copied_object_path": copied_object.as_ref().map(|path| path.display().to_string()),
        "module_context": "embedded-in-job",
        "replay": {
            "environment": job.codegen_environment.values,
            "environment_null_means": "unset before replay",
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
