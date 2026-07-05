use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::io_limits::ensure_output_parent_dir;
use super::super::{
    compile_stdlib_cache_object, publish_shared_stdlib_cache_object, stdlib_cache_temp_publish_path,
};
use super::request::NativeStdlibCachePrepare;

pub(crate) fn materialize_missing_stdlib_cache(
    ir: &mut SimpleIR,
    stdlib_path: &Path,
    request: &NativeStdlibCachePrepare<'_>,
    current_partition_manifest: &str,
    user_remaining: &mut Vec<molt_backend::FunctionIR>,
    stdlib_funcs: &mut Vec<molt_backend::FunctionIR>,
) -> io::Result<()> {
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
        std::mem::take(stdlib_funcs),
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
        current_partition_manifest,
    ) {
        let _ = std::fs::remove_file(&temp_stdlib_path);
        return Err(io::Error::new(
            err.kind(),
            format!("failed to publish shared stdlib cache: {err}"),
        ));
    }

    ir.functions = std::mem::take(user_remaining);
    eprintln!(
        "{}: compiling {} user functions",
        request.log_prefix,
        ir.functions.len()
    );
    Ok(())
}
