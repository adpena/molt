use std::{collections::BTreeSet, path::PathBuf};

use molt_backend::{ModuleRegistryIR, NativeBackendModuleContext, SimpleIR};

pub(crate) struct NativeApplicationArtifactOptions<'a> {
    pub(crate) native_output_kind: crate::backend_process::NativeArtifactKind,
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_split_enabled: bool,
    pub(crate) app_callable_manifest: Option<BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock, design doc 69): its init
    /// symbols are dead-function-elimination roots and the main application
    /// object emits its blob (`molt_module_registry_blob`).
    pub(crate) module_registry: Option<ModuleRegistryIR>,
    /// Whole-program ABI authority captured before stdlib extraction or batch
    /// partitioning removes provider bodies.
    pub(crate) module_context: Option<NativeBackendModuleContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeApplicationArtifactResult {
    pub(crate) function_count: usize,
    pub(crate) batch_count: usize,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchObjectJob {
    pub(crate) ir: SimpleIR,
    pub(crate) module_context: NativeBackendModuleContext,
    pub(crate) codegen_environment: molt_ir::backend_environment::NativeCodegenEnvironment,
    pub(crate) target_triple: Option<String>,
    pub(crate) emit_app_callable_resolver: bool,
    pub(crate) app_callable_manifest: Option<BTreeSet<String>>,
    pub(crate) external_function_names: BTreeSet<String>,
    /// Carried by the batch that emits the app callable resolver (the main
    /// application object): that batch also emits the module registry blob.
    #[serde(default)]
    pub(crate) module_registry: Option<ModuleRegistryIR>,
}

impl NativeBatchObjectJob {
    /// Close the object input contract before serialization. The worker receives
    /// exactly this projection, with no out-of-band whole-program context path.
    pub(crate) fn close_dependencies(mut self) -> Self {
        let mut names = NativeBackendModuleContext::object_dependencies(&self.ir.functions);
        if let Some(manifest) = &self.app_callable_manifest {
            names.extend(manifest.iter().cloned());
        }
        if let Some(registry) = &self.module_registry {
            names.extend(registry.init_symbols.iter().cloned());
            names.extend(registry.relocs.iter().map(|(_, name)| name.clone()));
        }
        self.module_context = self.module_context.project_object_dependencies(&names);
        self.external_function_names
            .retain(|name| names.contains(name));
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NativeBatchJobSpec {
    pub(crate) job_path: PathBuf,
    pub(crate) object_path: PathBuf,
}
