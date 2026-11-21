//! The JIT consumes the AOT stdlib partition without emitting app machine code.

use crate::backend_process::cli_args::NativeTargetOptions;
use crate::backend_process::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_artifact,
    read_stdlib_cache_partition_manifest,
};
use molt_backend::{BackendIrDocument, SimpleBackend};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::io;
use std::path::Path;

pub(crate) fn prepare(
    document: BackendIrDocument,
    options: NativeTargetOptions,
    output: &Path,
    program_storage: &Path,
) -> io::Result<()> {
    let BackendIrDocument {
        mut ir,
        module_registry,
        code_slots,
    } = document;
    let archive = super::shared_stdlib_archive_path_from_env()?
        .ok_or_else(|| io::Error::other("JIT preparation requires a shared stdlib archive"))?;
    let key = backend_env::var(BackendEnv::MOLT_STDLIB_CACHE_KEY).ok();
    let manifest = backend_env::var(BackendEnv::MOLT_STDLIB_CACHE_MANIFEST).ok();
    let entry = backend_env::var(BackendEnv::MOLT_ENTRY_MODULE).map_err(|err| {
        io::Error::other(format!("JIT preparation requires an entry module: {err}"))
    })?;
    let modules = molt_backend::stdlib_modules_from_env()
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("JIT preparation requires explicit stdlib modules"))?;
    let mut complete = ir.clone();
    let options = prepare_native_application_artifact(
        &mut ir,
        NativeStdlibCachePrepare {
            code_slots,
            native_output_kind: options.output_kind,
            target_triple: options.target_triple.as_deref(),
            stdlib_archive_path: Some(&archive),
            expected_cache_key: key.as_deref(),
            expected_cache_manifest: manifest.as_deref(),
            have_entry_module: true,
            entry_module: &entry,
            explicit_stdlib_modules: Some(&modules),
            log_prefix: "MOLT_BACKEND(JIT)",
            module_registry,
        },
    )?;
    let registry = options
        .module_registry
        .as_ref()
        .ok_or_else(|| io::Error::other("JIT application has no module registry"))?;
    let storage = SimpleBackend::emit_native_program_storage(
        options.target_triple,
        &options.code_slots,
        registry,
    );
    molt_artifact_publish::write_bytes_atomically(program_storage, &storage)?;
    let partition = read_stdlib_cache_partition_manifest(Path::new(&archive))
        .ok_or_else(|| io::Error::other("missing admitted JIT stdlib partition"))?;
    #[derive(serde::Deserialize)]
    struct PartitionBodies {
        functions: std::collections::BTreeSet<String>,
    }
    let shared_stdlib = serde_json::from_str::<PartitionBodies>(&partition)
        .map_err(io::Error::other)?
        .functions;
    molt_backend::prepare_native_application_ir(
        &mut complete,
        options.module_registry.as_ref(),
        &options.code_slots,
        true,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    let mut compiler = SimpleBackend::new_with_target(options.target_triple);
    compiler.module_registry = options.module_registry;
    compiler.code_slots = Some(options.code_slots);
    compiler.app_callable_manifest = options.app_callable_manifest;
    if let Some(context) = options.module_context {
        compiler.set_module_context(context);
    }
    let application = compiler
        .prepare_jit_application(complete, &shared_stdlib)
        .map_err(io::Error::other)?;
    let program = application.encode().map_err(io::Error::other)?;
    molt_artifact_publish::write_bytes_atomically(output, &program)
}
