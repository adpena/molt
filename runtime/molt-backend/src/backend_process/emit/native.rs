use molt_backend::SimpleIR;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::io;
use std::path::Path;

use super::super::native_batch::compile_native_application_artifact_to_path;
use super::super::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_artifact,
};

pub(super) fn emit_native_target(
    mut ir: SimpleIR,
    module_registry: Option<molt_backend::ModuleRegistryIR>,
    code_slots: molt_ir::code_slots::CodeSlotBindings,
    output_file: &str,
    target_triple: Option<&str>,
    native_output_kind: super::super::NativeArtifactKind,
) -> io::Result<crate::backend_process::origin::EmittedArtifact> {
    let stdlib_archive_path = super::super::shared_stdlib_archive_path_from_env()?;
    let cache_archive_path =
        super::super::native_artifact::shared_stdlib_cache_entry_path_from_env()?;
    let expected_stdlib_cache_key = backend_env::var(BackendEnv::MOLT_STDLIB_CACHE_KEY).ok();
    let expected_stdlib_cache_manifest =
        backend_env::var(BackendEnv::MOLT_STDLIB_CACHE_MANIFEST).ok();
    let have_entry_module = backend_env::var(BackendEnv::MOLT_ENTRY_MODULE).is_ok();
    let entry_module =
        backend_env::var(BackendEnv::MOLT_ENTRY_MODULE).unwrap_or_else(|_| "__main__".to_string());
    let explicit_stdlib_module_symbols = molt_backend::stdlib_module_symbols_from_env()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let compile_options = prepare_native_application_artifact(
        &mut ir,
        NativeStdlibCachePrepare {
            native_output_kind,
            target_triple,
            stdlib_archive_path: stdlib_archive_path.as_deref(),
            cache_archive_path: cache_archive_path.as_deref(),
            expected_cache_key: expected_stdlib_cache_key.as_deref(),
            expected_cache_manifest: expected_stdlib_cache_manifest.as_deref(),
            have_entry_module,
            entry_module: &entry_module,
            explicit_stdlib_module_symbols: explicit_stdlib_module_symbols.as_ref(),
            log_prefix: "MOLT_BACKEND",
            module_registry,
            code_slots,
        },
    )?;

    let emitted =
        compile_native_application_artifact_to_path(ir, Path::new(output_file), compile_options)?;
    let codegen = match emitted.workers {
        Some(workers) => crate::backend_process::origin::CodegenOrigin::NativeWorkers { workers },
        None => crate::backend_process::origin::CodegenOrigin::in_process(),
    };
    Ok(crate::backend_process::origin::EmittedArtifact {
        output: emitted.output,
        codegen,
    })
}
