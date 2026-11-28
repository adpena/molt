use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::native_batch::NativeApplicationArtifactOptions;

mod materialize;
#[cfg(test)]
pub(crate) use materialize::MATERIALIZED;
mod partition;
mod request;

pub(crate) use request::NativeStdlibCachePrepare;

pub(crate) fn prepare_native_application_artifact<'a>(
    ir: &mut SimpleIR,
    request: NativeStdlibCachePrepare<'a>,
) -> io::Result<NativeApplicationArtifactOptions<'a>> {
    request
        .code_slots
        .validate_functions(&ir.functions)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    molt_ir::ir::validate_module_slots(request.module_registry.as_ref(), &ir.functions)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    request
        .native_output_kind
        .validate_shared_stdlib(request.stdlib_archive_path.is_some(), request.target_triple)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    super::super::native_artifact::validate_shared_stdlib_cache_output(
        request.stdlib_archive_path,
    )?;
    let mut module_context = None;
    let mut source_facts = None;
    let mut app_callable_manifest = None;
    let mut object_cache = None;
    let module_registry_roots: std::collections::BTreeSet<String> = request
        .module_registry
        .as_ref()
        .map(|registry| registry.init_symbols.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(stdlib_path_str) = request.stdlib_archive_path {
        let stdlib_path = Path::new(stdlib_path_str);
        object_cache = super::NativeObjectCache::for_archive(
            stdlib_path,
            request.expected_cache_key,
            request.expected_cache_manifest,
            request.target_triple,
        )?;
        let mut prepared =
            partition::prepare_stdlib_partition(ir, &request, &module_registry_roots, stdlib_path)?;
        module_context = Some(prepared.module_context.clone());
        let stdlib_count = prepared.stdlib_funcs.len();
        let can_reuse = request.can_reuse_stdlib_cache();
        let reuse = || {
            if !can_reuse {
                return Ok(false);
            }
            super::admit_or_invalidate_shared_stdlib_cache(
                stdlib_path,
                request.expected_cache_key,
                request.expected_cache_manifest,
                &prepared.current_partition_manifest,
                stdlib_count,
                request.log_prefix,
            )
        };
        // Warm builds reuse without waiting. On a miss, builds that need the
        // same shared entry take its producer lock and check again: one
        // compiles, and the rest hydrate the archive it published.
        let mut reused = reuse()?;
        if !reused {
            let mut produce = || -> io::Result<bool> {
                if reuse()? {
                    return Ok(true);
                }
                materialize::materialize_stdlib_cache(
                    ir,
                    stdlib_path,
                    &request,
                    &prepared.current_partition_manifest,
                    &mut prepared.stdlib_funcs,
                    &prepared.module_context,
                )?;
                Ok(false)
            };
            // Only a build that could reuse the entry waits for its producer;
            // the others compile in parallel, as they must compile anyway.
            reused = if can_reuse {
                super::with_shared_stdlib_cache_producer_lock(
                    &super::resolved_archive_path(stdlib_path)?,
                    produce,
                )?
            } else {
                produce()?
            };
        }
        // Partitioning owns the application document, including its referenced
        // extern rows. Whether the stdlib archive needed production cannot add
        // declarations to that document or change its action identity.
        ir.functions = prepared.user_remaining;
        source_facts = prepared.user_facts;
        let extern_count = ir
            .functions
            .iter()
            .filter(|function| function.is_extern)
            .count();
        let user_count = ir.functions.len() - extern_count;
        let log_prefix = request.log_prefix;
        if reused {
            eprintln!(
                "{log_prefix}: incremental -- compiling {user_count} user functions \
                 ({extern_count} stdlib extern from {})",
                stdlib_path.display()
            );
        } else {
            eprintln!("{log_prefix}: compiling {user_count} user functions");
        }
        app_callable_manifest = Some(prepared.app_callable_manifest);
    }

    Ok(NativeApplicationArtifactOptions {
        object_cache,
        native_output_kind: request.native_output_kind,
        target_triple: request.target_triple,
        stdlib_split_enabled: request.stdlib_archive_path.is_some(),
        app_callable_manifest,
        log_prefix: request.log_prefix,
        module_registry: request.module_registry,
        code_slots: request.code_slots,
        module_context,
        source_facts,
    })
}
