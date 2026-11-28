use std::io;
use std::path::Path;

use molt_backend::tir::cache::{CompilationCache, CompilationCacheWriteError};
use molt_backend::{ModuleRegistryIR, NativeBackendModuleContext, SimpleIR};
use molt_ir::backend_environment::NativeCodegenEnvironment;
use molt_ir::code_slots::CodeSlotBindings;
use serde::Serialize;

use super::super::super::{NativeApplicationArtifactOptions, NativeApplicationArtifactResult};
use crate::backend_process::shared_stdlib_cache::NativeObjectCache;

/// All inputs consumed before constituent keys exist. Keep the unspecialized
/// body and the complete provider context here: module analysis can discover
/// dependencies which are absent from source-level direct calls.
#[derive(Serialize)]
struct ApplicationPlanInput<'a> {
    ir: &'a SimpleIR,
    target: Option<&'a str>,
    module_context: Option<&'a NativeBackendModuleContext>,
    app_callable_manifest: Option<&'a molt_backend::passes::AppCallableManifest>,
    module_registry: Option<&'a ModuleRegistryIR>,
    code_slots: &'a CodeSlotBindings,
    environment: &'a NativeCodegenEnvironment,
    batch_size: usize,
    batch_ops_budget: usize,
}

/// A plan owns only an ordered list of existing object commitments. A missing
/// constituent falls through to the normal compiler, where every surviving
/// object is still reusable. Old records and objects retire independently.
pub(super) struct ApplicationPlanRecord {
    key: String,
    store: CompilationCache,
}

impl ApplicationPlanRecord {
    pub(super) fn new(
        cache: &NativeObjectCache,
        ir: &SimpleIR,
        options: &NativeApplicationArtifactOptions<'_>,
        environment: &NativeCodegenEnvironment,
        batch_size: usize,
        batch_ops_budget: usize,
    ) -> io::Result<Self> {
        // Every option requires an explicit decision here. Artifact kind and
        // stdlib selection already acted on the admitted IR and routed this
        // request to the batch archive producer. The store and log sink cannot
        // affect that producer's objects.
        let NativeApplicationArtifactOptions {
            object_cache: _,
            code_slots,
            native_output_kind: _,
            target_triple,
            stdlib_split_enabled: _,
            app_callable_manifest,
            log_prefix: _,
            module_registry,
            module_context,
        } = options;
        let key = cache.application_plan_key(&ApplicationPlanInput {
            ir,
            target: *target_triple,
            module_context: module_context.as_ref(),
            app_callable_manifest: app_callable_manifest.as_ref(),
            module_registry: module_registry.as_ref(),
            code_slots,
            environment,
            batch_size,
            batch_ops_budget,
        })?;
        Ok(Self {
            key,
            store: cache.application_plan_store(),
        })
    }

    pub(super) fn reuse(
        &mut self,
        cache: &NativeObjectCache,
        output_path: &Path,
        function_count: usize,
        log_prefix: &str,
    ) -> io::Result<Option<NativeApplicationArtifactResult>> {
        let Some(bytes) = self.store.get(&self.key) else {
            return Ok(None);
        };
        let (keys, remainder) = bytes.as_chunks::<32>();
        if keys.is_empty() || !remainder.is_empty() {
            return Err(io::Error::other("invalid native application archive plan"));
        }
        let objects =
            molt_backend::tir::pipeline_cache::request_parallel_map(keys, |key| cache.admit(key))
                .into_iter()
                .collect::<io::Result<Option<Vec<_>>>>()?;
        let Some(objects) = objects else {
            return Ok(None);
        };
        let (objects, origins): (Vec<_>, Vec<_>) = objects.into_iter().unzip();
        let output = super::super::super::write_native_archive_objects(output_path, &objects)?;
        eprintln!(
            "{log_prefix}: application archive plan reuse ({} objects, no terminal TIR preparation)",
            keys.len()
        );
        Ok(Some(NativeApplicationArtifactResult {
            function_count,
            batch_count: keys.len(),
            codegen: crate::backend_process::origin::CodegenOrigin::NativeWorkers {
                workers: origins,
            },
            output,
        }))
    }

    pub(super) fn publish(&mut self, keys: &[[u8; 32]]) -> io::Result<()> {
        assert!(!keys.is_empty(), "a native archive has at least one object");
        match self.store.put(&self.key, keys.as_flattened()) {
            Ok(()) => Ok(()),
            Err(CompilationCacheWriteError::Integrity(error)) => Err(io::Error::other(error)),
            Err(CompilationCacheWriteError::Unavailable(error)) => {
                eprintln!("MOLT_CACHE: native application archive plan unavailable: {error}");
                Ok(())
            }
        }
    }
}
