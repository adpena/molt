use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::native_batch::NativeApplicationArtifactOptions;

mod materialize;
mod partition;
mod request;
mod reuse;

pub(crate) use request::NativeStdlibCachePrepare;

pub(crate) fn prepare_native_application_artifact<'a>(
    ir: &mut SimpleIR,
    request: NativeStdlibCachePrepare<'a>,
) -> io::Result<NativeApplicationArtifactOptions<'a>> {
    request
        .native_output_kind
        .validate_shared_stdlib(request.stdlib_archive_path.is_some())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut module_context = None;
    let app_callable_manifest = request
        .stdlib_archive_path
        .map(|_| molt_backend::compute_app_callable_manifest_checked(&ir.functions));
    let module_registry_roots: std::collections::BTreeSet<String> = request
        .module_registry
        .as_ref()
        .map(|registry| registry.init_symbols.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(stdlib_path_str) = request.stdlib_archive_path {
        let stdlib_path = Path::new(stdlib_path_str);
        let mut prepared =
            partition::prepare_stdlib_partition(ir, &request, &module_registry_roots, stdlib_path)?;
        module_context = Some(prepared.module_context.clone());
        let reused = reuse::try_reuse_existing_stdlib_cache(
            ir,
            stdlib_path,
            &request,
            &prepared.current_partition_manifest,
            &mut prepared.user_remaining,
            &mut prepared.stdlib_funcs,
        )?;
        if !reused {
            materialize::materialize_stdlib_cache(
                ir,
                stdlib_path,
                &request,
                &prepared.current_partition_manifest,
                &mut prepared.user_remaining,
                &mut prepared.stdlib_funcs,
                &prepared.module_context,
            )?;
        }
    }

    Ok(NativeApplicationArtifactOptions {
        native_output_kind: request.native_output_kind,
        target_triple: request.target_triple,
        stdlib_split_enabled: request.stdlib_archive_path.is_some(),
        app_callable_manifest,
        log_prefix: request.log_prefix,
        module_registry: request.module_registry,
        module_context,
    })
}
