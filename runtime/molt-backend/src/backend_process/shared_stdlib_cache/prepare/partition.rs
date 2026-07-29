use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::io_limits::ensure_output_parent_dir;
use super::super::{
    prune_and_partition_native_stdlib, remove_shared_stdlib_cache_artifacts,
    shared_stdlib_partition_manifest, shared_stdlib_split_function_names,
    validate_shared_stdlib_partition,
};
use super::request::NativeStdlibCachePrepare;

pub(crate) struct PreparedStdlibPartition {
    pub(crate) user_remaining: Vec<molt_backend::FunctionIR>,
    pub(crate) stdlib_funcs: Vec<molt_backend::FunctionIR>,
    pub(crate) current_partition_manifest: String,
    pub(crate) module_context: molt_backend::NativeBackendModuleContext,
}

pub(crate) fn prepare_stdlib_partition(
    ir: &mut SimpleIR,
    request: &NativeStdlibCachePrepare<'_>,
    module_registry_roots: &std::collections::BTreeSet<String>,
    stdlib_path: &Path,
) -> io::Result<PreparedStdlibPartition> {
    let (user_remaining, stdlib_funcs, module_context) = prune_and_partition_native_stdlib(
        ir,
        request.entry_module,
        request.explicit_stdlib_module_symbols,
        module_registry_roots,
    );
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(|err| {
        eprintln!(
            "{}: warning: failed to create stdlib parent: {err}",
            request.log_prefix
        );
    });

    let current_partition_manifest =
        shared_stdlib_partition_manifest(&stdlib_funcs, &module_context).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to compute shared stdlib partition manifest: {err}"),
            )
        })?;
    let split_function_names = shared_stdlib_split_function_names(&user_remaining, &stdlib_funcs);
    if let Err(err) = validate_shared_stdlib_partition(&stdlib_funcs, &split_function_names) {
        remove_shared_stdlib_cache_artifacts(stdlib_path);
        return Err(io::Error::new(
            err.kind(),
            format!("invalid shared stdlib partition: {err}"),
        ));
    }

    Ok(PreparedStdlibPartition {
        user_remaining,
        stdlib_funcs,
        current_partition_manifest,
        module_context,
    })
}
