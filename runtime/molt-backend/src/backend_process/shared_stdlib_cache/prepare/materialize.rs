use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::super::native_batch::write_native_archive_objects;
use super::super::{compile_stdlib_cache_archive, publish_shared_stdlib_cache_archive};
use super::request::NativeStdlibCachePrepare;

pub(crate) fn materialize_stdlib_cache(
    ir: &mut SimpleIR,
    stdlib_path: &Path,
    request: &NativeStdlibCachePrepare<'_>,
    current_partition_manifest: &str,
    user_remaining: &mut Vec<molt_backend::FunctionIR>,
    stdlib_funcs: &mut Vec<molt_backend::FunctionIR>,
    module_context: &molt_backend::NativeBackendModuleContext,
) -> io::Result<()> {
    let stdlib_count = stdlib_funcs.len();
    eprintln!(
        "{}: materializing {} stdlib functions to {}",
        request.log_prefix,
        stdlib_count,
        stdlib_path.display()
    );
    let cache_path = request
        .cache_archive_path
        .map(|path| super::super::publish::resolved_archive_path(Path::new(path)))
        .transpose()?;
    let object_cache = super::super::publish::StdlibObjectCache::for_archive(
        cache_path.as_deref().unwrap_or(stdlib_path),
        request.expected_cache_key,
        request.expected_cache_manifest,
        request.target_triple,
    )?;
    // Codegen and constituent reuse run in native-batch scratch. Only the
    // final archive assembly/sidecars acquire the shared publication stripe.
    compile_stdlib_cache_archive(
        std::mem::take(stdlib_funcs),
        ir.profile.clone(),
        request.target_triple,
        request.log_prefix,
        module_context.clone(),
        object_cache.as_ref(),
        |objects| {
            publish_shared_stdlib_cache_archive(
                stdlib_path,
                |destination| write_native_archive_objects(destination, objects),
                stdlib_count,
                request.expected_cache_key,
                request.expected_cache_manifest,
                current_partition_manifest,
            )
        },
    )?;

    if let Some(cache_path) = cache_path.as_deref() {
        // Required bytes remain in build custody throughout optional cache
        // publication. Eviction after this stripe releases cannot break linking.
        publish_shared_stdlib_cache_archive(
            cache_path,
            |destination| {
                molt_artifact_publish::write_atomically(destination, |writer| {
                    let mut source = std::fs::File::open(stdlib_path)?;
                    io::copy(&mut source, writer).map(|_| ())
                })
            },
            stdlib_count,
            request.expected_cache_key,
            request.expected_cache_manifest,
            current_partition_manifest,
        )?;
    }

    ir.functions = std::mem::take(user_remaining);
    eprintln!(
        "{}: compiling {} user functions",
        request.log_prefix,
        ir.functions.len()
    );
    Ok(())
}
