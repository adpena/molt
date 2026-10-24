use std::{io, path::Path};

use molt_backend::SimpleBackend;

use super::super::super::io_limits::write_output_path;
use super::super::batching::NativeBatchObjectJob;
use crate::backend_process::input::content::{ContentIdentity, ContentReader};

/// Worker-owned observation emitted only after complete input decoding and output publication.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeWorkerOrigin {
    pub(crate) schema: String,
    pub(crate) process_id: u32,
    pub(crate) scope: Option<String>,
    pub(crate) input: ContentIdentity,
    pub(crate) output: ContentIdentity,
}
impl NativeWorkerOrigin {
    pub(crate) fn validate(&self) -> io::Result<()> {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.schema != "molt.native-worker-origin.v2"
            || self.process_id == 0
            || self.scope.as_deref().is_some_and(|scope| !digest(scope))
            || self.input.size == 0
            || self.output.size == 0
            || !digest(&self.input.sha256)
            || !digest(&self.output.sha256)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native worker origin is invalid",
            ));
        }
        Ok(())
    }

    #[cfg(molt_backend_daemon)]
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.schema.capacity()
            + self.scope.as_ref().map_or(0, String::capacity)
            + self.input.sha256.capacity()
            + self.output.sha256.capacity()
    }
}

fn compile_native_batch_object_job(
    job: NativeBatchObjectJob,
    output_path: &Path,
) -> io::Result<ContentIdentity> {
    if job.emit_app_callable_resolver != job.code_slots.is_some()
        || (!job.emit_app_callable_resolver && job.module_registry.is_some())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native batch program-slot bindings must belong exactly to the app metadata owner",
        ));
    }
    job.codegen_environment.validate_current()?;
    let mut backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
    backend.skip_ir_passes = true;
    backend.skip_shared_stdlib_partition = true;
    backend.emit_app_callable_resolver = job.emit_app_callable_resolver;
    backend.app_callable_manifest = job.app_callable_manifest;
    backend.external_function_names = job.external_function_names;
    backend.module_registry = job.module_registry;
    backend.code_slots = job.code_slots;
    backend.set_module_context(job.module_context);
    let output = backend.compile(job.ir);
    write_output_path(output_path, &output.bytes)?;
    Ok(ContentIdentity::of_bytes(&output.bytes))
}

/// `--native-batch-job-file JOB`: JOB names a preserved replay file, or `-` for
/// the job its parent streams on stdin.
pub(crate) fn compile_native_batch_object_job_file(
    job_path: &Path,
    output_path: &Path,
) -> io::Result<NativeWorkerOrigin> {
    if job_path == Path::new("-") {
        decode_compile_native_worker(io::stdin().lock(), output_path)
    } else {
        let file = molt_artifact_publish::open_regular_artifact(job_path)?;
        decode_compile_native_worker(file, output_path)
    }
}

fn decode_compile_native_worker(
    reader: impl io::Read,
    output_path: &Path,
) -> io::Result<NativeWorkerOrigin> {
    let mut reader = ContentReader::new(reader);
    // serde_json::from_reader requires EOF: rejected suffixes never produce an origin.
    let job: NativeBatchObjectJob = serde_json::from_reader(io::BufReader::new(&mut reader))
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid native batch object job: {err}"),
            )
        })?;
    let input = reader.identity();
    let output = compile_native_batch_object_job(job, output_path)?;
    Ok(NativeWorkerOrigin {
        schema: "molt.native-worker-origin.v2".into(),
        scope: crate::backend_process::origin::current_scope().map(str::to_owned),
        process_id: std::process::id(),
        input,
        output,
    })
}

#[cfg(test)]
pub(crate) fn compile_native_batch_object_job_bytes(
    job: &[u8],
    output_path: &Path,
) -> io::Result<NativeWorkerOrigin> {
    decode_compile_native_worker(job, output_path)
}
