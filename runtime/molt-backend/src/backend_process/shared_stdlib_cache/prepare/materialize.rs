use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::io_limits::ensure_output_parent_dir;
use super::super::{
    compile_stdlib_cache_archive, publish_shared_stdlib_cache_archive,
    stdlib_cache_temp_publish_path,
};
use super::request::NativeStdlibCachePrepare;
use molt_artifact_publish::cleanup_temporary_after_error;

pub(crate) fn materialize_stdlib_cache(
    ir: &mut SimpleIR,
    stdlib_path: &Path,
    request: &NativeStdlibCachePrepare<'_>,
    current_partition_manifest: &str,
    user_remaining: &mut Vec<molt_backend::FunctionIR>,
    stdlib_funcs: &mut Vec<molt_backend::FunctionIR>,
    module_context: &molt_backend::NativeBackendModuleContext,
) -> io::Result<()> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or("")).unwrap_or_else(|err| {
        eprintln!(
            "{}: warning: could not create stdlib cache parent dir: {err}",
            request.log_prefix
        );
    });

    let stdlib_count = stdlib_funcs.len();
    eprintln!(
        "{}: materializing {} stdlib functions to {}",
        request.log_prefix,
        stdlib_count,
        stdlib_path.display()
    );
    let temp_stdlib_path = stdlib_cache_temp_publish_path(stdlib_path, "archive");
    if let Err(err) = compile_stdlib_cache_archive(
        &temp_stdlib_path,
        std::mem::take(stdlib_funcs),
        ir.profile.clone(),
        request.target_triple,
        request.log_prefix,
        module_context.clone(),
    ) {
        return Err(cleanup_temporary_after_error(&temp_stdlib_path, err));
    }
    if let Err(err) = publish_shared_stdlib_cache_archive(
        stdlib_path,
        &temp_stdlib_path,
        stdlib_count,
        request.expected_cache_key,
        request.expected_cache_manifest,
        current_partition_manifest,
    ) {
        // The publisher owns temporary cleanup and preserves any cleanup
        // failure in the returned publication error.
        return Err(err);
    }

    ir.functions = std::mem::take(user_remaining);
    eprintln!(
        "{}: compiling {} user functions",
        request.log_prefix,
        ir.functions.len()
    );
    Ok(())
}
