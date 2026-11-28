use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::io_limits::ensure_output_parent_dir;
use super::super::{
    prune_and_partition_native_stdlib, shared_stdlib_partition_manifest,
    shared_stdlib_split_function_names, validate_shared_stdlib_partition,
};
use super::request::NativeStdlibCachePrepare;

pub(crate) struct PreparedStdlibPartition {
    pub(crate) user_remaining: Vec<molt_backend::FunctionIR>,
    pub(crate) stdlib_funcs: Vec<molt_backend::FunctionIR>,
    pub(crate) current_partition_manifest: String,
    pub(crate) module_context: molt_backend::NativeBackendModuleContext,
    pub(crate) app_callable_manifest: molt_backend::passes::AppCallableManifest,
}

pub(crate) fn prepare_stdlib_partition(
    ir: &mut SimpleIR,
    request: &NativeStdlibCachePrepare<'_>,
    module_registry_roots: &std::collections::BTreeSet<String>,
    stdlib_path: &Path,
) -> io::Result<PreparedStdlibPartition> {
    let partition = prune_and_partition_native_stdlib(
        ir,
        request.entry_module,
        request.explicit_stdlib_modules,
        module_registry_roots,
        molt_backend::tir::cache::CompilationCache::open_backend_namespace().as_mut(),
    );
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or(""))?;

    let current_partition_manifest = shared_stdlib_partition_manifest(
        &partition.stdlib_funcs,
        partition.stdlib_contracts.as_deref(),
        &partition.module_context,
    )
    .map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to compute shared stdlib partition manifest: {err}"),
        )
    })?;
    let split_function_names =
        shared_stdlib_split_function_names(&partition.user_remaining, &partition.stdlib_funcs);
    if let Err(err) =
        validate_shared_stdlib_partition(&partition.stdlib_funcs, &split_function_names)
    {
        return Err(io::Error::new(
            err.kind(),
            format!("invalid shared stdlib partition: {err}"),
        ));
    }

    Ok(PreparedStdlibPartition {
        user_remaining: partition.user_remaining,
        stdlib_funcs: partition.stdlib_funcs,
        current_partition_manifest,
        module_context: partition.module_context,
        app_callable_manifest: partition.app_callable_manifest,
    })
}
