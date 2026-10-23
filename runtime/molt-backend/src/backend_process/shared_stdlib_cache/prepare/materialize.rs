use std::io;
use std::path::Path;
use std::time::Instant;

use molt_backend::SimpleIR;

use super::super::super::native_batch::write_native_archive_objects;
use super::super::publish::{
    ArchiveGeneration, ContentArchive, StdlibObjectCache, hydrate_shared_stdlib_cache_archive,
};
use super::super::{plan_stdlib_cache_archive, publish_shared_stdlib_cache_archive};
use super::request::NativeStdlibCachePrepare;

/// Materializations per cache key, so tests can prove single production.
#[cfg(test)]
pub(crate) static MATERIALIZED: std::sync::Mutex<std::collections::BTreeMap<String, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

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
    #[cfg(test)]
    {
        let key = request.expected_cache_key.unwrap_or_default().to_string();
        *MATERIALIZED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key)
            .or_default() += 1;
    }
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
    let object_cache = StdlibObjectCache::for_archive(
        cache_path.as_deref().unwrap_or(stdlib_path),
        request.expected_cache_key,
        request.expected_cache_manifest,
        request.target_triple,
    )?;
    let started = Instant::now();
    let planned = plan_stdlib_cache_archive(
        std::mem::take(stdlib_funcs),
        ir.profile.clone(),
        request.target_triple,
        module_context.clone(),
        object_cache.as_ref(),
    )?;
    let content = object_cache
        .as_ref()
        .zip(planned.archive_key())
        .map(|(cache, key)| cache.content_archive(&key))
        .transpose()?;
    let plan_elapsed = started.elapsed();
    let build = ArchiveGeneration {
        path: stdlib_path,
        key: request.expected_cache_key,
        manifest: request.expected_cache_manifest,
    };
    // An unchanged partition is one archive generation for every program that
    // compiles it: transfer it whole. Codegen and constituent reuse run in
    // native-batch scratch otherwise; only archive publication takes a stripe.
    let transfer_started = Instant::now();
    let reused = match &content {
        Some(content) => hydrate_shared_stdlib_cache_archive(
            content_generation(content),
            build,
            current_partition_manifest,
            stdlib_count,
        )?,
        None => None,
    };
    let digest = match reused {
        Some(digest) => {
            eprintln!(
                "{}: stdlib archive reuse: content {} ({} functions planned in {:?}, transferred in {:?})",
                request.log_prefix,
                content.as_ref().expect("reused content").key,
                stdlib_count,
                plan_elapsed,
                transfer_started.elapsed(),
            );
            drop(planned);
            digest
        }
        None => {
            let mut digest = None;
            planned.assemble(request.log_prefix, object_cache.as_ref(), |objects| {
                digest = Some(publish_shared_stdlib_cache_archive(
                    stdlib_path,
                    |destination| write_native_archive_objects(destination, objects).map(|()| None),
                    stdlib_count,
                    request.expected_cache_key,
                    request.expected_cache_manifest,
                    current_partition_manifest,
                )?);
                Ok(())
            })?;
            let digest = digest.expect("assembly publishes the archive");
            if let Some(content) = &content {
                // Later programs with this partition transfer the generation.
                publish_copy(
                    stdlib_path,
                    &digest,
                    content_generation(content),
                    stdlib_count,
                    current_partition_manifest,
                )?;
            }
            digest
        }
    };

    if let Some(cache_path) = cache_path.as_deref() {
        // Required bytes remain in build custody throughout optional cache
        // publication. Eviction after this stripe releases cannot break linking.
        publish_copy(
            stdlib_path,
            &digest,
            ArchiveGeneration {
                path: cache_path,
                key: request.expected_cache_key,
                manifest: request.expected_cache_manifest,
            },
            stdlib_count,
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

fn content_generation(content: &ContentArchive) -> ArchiveGeneration<'_> {
    ArchiveGeneration {
        path: &content.path,
        key: Some(&content.key),
        manifest: Some(&content.manifest),
    }
}

/// Publish a copy of the build's archive, whose `digest` this build recorded,
/// as the cache generation `destination`. The platform copy clones where the
/// volume supports it, so no archive bytes are rewritten.
fn publish_copy(
    archive: &Path,
    digest: &str,
    destination: ArchiveGeneration<'_>,
    stdlib_count: usize,
    partition: &str,
) -> io::Result<()> {
    publish_shared_stdlib_cache_archive(
        destination.path,
        |target| {
            molt_artifact_publish::copy_file_atomically(archive, target, |_| Ok(()))
                .map(|()| Some(digest.to_string()))
        },
        stdlib_count,
        destination.key,
        destination.manifest,
        partition,
    )
    .map(drop)
}
