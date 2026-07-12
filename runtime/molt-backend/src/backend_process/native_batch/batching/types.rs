use std::{collections::BTreeSet, path::PathBuf};

use molt_backend::{ModuleRegistryIR, NativeBackendModuleContext, SimpleIR};

pub(crate) struct NativeApplicationObjectOptions<'a> {
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_split_enabled: bool,
    pub(crate) app_callable_manifest: Option<BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock, design doc 69): its init
    /// symbols are dead-function-elimination roots and the main application
    /// object emits its blob (`molt_module_registry_blob`).
    pub(crate) module_registry: Option<ModuleRegistryIR>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeApplicationObjectResult {
    pub(crate) function_count: usize,
    pub(crate) batch_count: usize,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchModuleMetadata {
    pub(crate) module_context: NativeBackendModuleContext,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchObjectJob {
    pub(crate) ir: SimpleIR,
    pub(crate) module_context_path: PathBuf,
    pub(crate) target_triple: Option<String>,
    pub(crate) emit_app_callable_resolver: bool,
    pub(crate) app_callable_manifest: Option<BTreeSet<String>>,
    pub(crate) external_function_names: BTreeSet<String>,
    /// Carried by the batch that emits the app callable resolver (the main
    /// application object): that batch also emits the module registry blob.
    #[serde(default)]
    pub(crate) module_registry: Option<ModuleRegistryIR>,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeBatchJobSpec {
    pub(crate) job_path: PathBuf,
    pub(crate) object_path: PathBuf,
}
