#[cfg(feature = "native-backend")]
use std::path::Path;
#[cfg(feature = "wasm-backend")]
use std::sync::Arc;

#[cfg(feature = "wasm-backend")]
use crate::backend_process::WasmTargetOptions;
#[cfg(feature = "wasm-backend")]
use crate::backend_process::emit::validate_wasm_module_catalog;
#[cfg(feature = "native-backend")]
use crate::backend_process::{NativeTargetOptions, shared_stdlib_archive_path_from_env};
#[cfg(feature = "wasm-backend")]
use molt_backend::WasmBackend;

#[cfg(feature = "native-backend")]
use super::super::super::native_batch::compile_native_application_artifact_to_path;
#[cfg(feature = "native-backend")]
use super::super::super::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_artifact, shared_stdlib_cache_matches,
};
use super::super::{DaemonJobRequest, DaemonJobTarget};

pub(crate) enum DaemonCompiledOutput {
    #[cfg(feature = "wasm-backend")]
    Bytes(Arc<[u8]>),
    #[cfg(feature = "native-backend")]
    WrittenToPath,
}

/// A job's target with the environment it compiles under resolved. A native
/// job may link a shared stdlib archive, which its output kind must admit,
/// and writes its artifact itself; the daemon writes WASM bytes for its job.
pub(super) enum ResolvedJobTarget {
    #[cfg(feature = "native-backend")]
    Native {
        options: NativeTargetOptions,
        stdlib_archive_path: Option<String>,
        output_path: String,
    },
    #[cfg(feature = "wasm-backend")]
    Wasm(WasmTargetOptions),
}

impl ResolvedJobTarget {
    pub(super) fn resolve(job: &DaemonJobRequest) -> Result<Self, String> {
        match &job.target {
            #[cfg(feature = "native-backend")]
            DaemonJobTarget::Native(options) => {
                let stdlib_archive_path =
                    shared_stdlib_archive_path_from_env().map_err(|error| error.to_string())?;
                options
                    .output_kind
                    .validate_shared_stdlib(stdlib_archive_path.is_some())?;
                Ok(Self::Native {
                    options: options.clone(),
                    stdlib_archive_path,
                    output_path: job.output.clone(),
                })
            }
            #[cfg(feature = "wasm-backend")]
            DaemonJobTarget::Wasm(options) => Ok(Self::Wasm(*options)),
        }
    }

    /// Whether the in-memory artifact cache may serve and keep this job's
    /// output. Requested compilation diagnostics must run the compiler, and a
    /// shared stdlib archive must still match the cache key it was built for.
    pub(super) fn memory_cache_allowed(&self) -> bool {
        if molt_ir::backend_environment::compilation_diagnostics_requested() {
            return false;
        }
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native {
                stdlib_archive_path: Some(stdlib_archive_path),
                ..
            } => shared_stdlib_cache_matches(
                Path::new(stdlib_archive_path),
                std::env::var("MOLT_STDLIB_CACHE_KEY").ok().as_deref(),
                std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok().as_deref(),
                None,
            ),
            #[cfg(feature = "native-backend")]
            Self::Native {
                stdlib_archive_path: None,
                ..
            } => true,
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(_) => true,
        }
    }

    pub(super) fn compile(
        self,
        document: molt_backend::BackendIrDocument,
    ) -> Result<DaemonCompiledOutput, String> {
        let molt_backend::BackendIrDocument {
            ir,
            module_registry,
        } = document;
        match self {
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(options) => {
                if let Some(registry) = module_registry.as_ref() {
                    validate_wasm_module_catalog(&ir, registry).map_err(|err| err.to_string())?;
                }
                let backend = WasmBackend::with_options(options.compile_options())
                    .with_module_registry(module_registry);
                Ok(DaemonCompiledOutput::Bytes(Arc::from(backend.compile(ir))))
            }
            #[cfg(feature = "native-backend")]
            Self::Native {
                options,
                stdlib_archive_path,
                output_path,
            } => {
                let mut ir = ir;
                let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
                let expected_stdlib_cache_manifest =
                    std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
                let entry_module =
                    std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
                let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
                let explicit_stdlib_module_symbols =
                    molt_backend::stdlib_module_symbols_from_env()?;
                let compile_options = prepare_native_application_artifact(
                    &mut ir,
                    NativeStdlibCachePrepare {
                        native_output_kind: options.output_kind,
                        target_triple: options.target_triple.as_deref(),
                        stdlib_archive_path: stdlib_archive_path.as_deref(),
                        expected_cache_key: expected_stdlib_cache_key.as_deref(),
                        expected_cache_manifest: expected_stdlib_cache_manifest.as_deref(),
                        have_entry_module,
                        entry_module: &entry_module,
                        explicit_stdlib_module_symbols: explicit_stdlib_module_symbols.as_ref(),
                        log_prefix: "MOLT_BACKEND(daemon)",
                        module_registry,
                    },
                )
                .map_err(|err| err.to_string())?;

                compile_native_application_artifact_to_path(
                    ir,
                    Path::new(&output_path),
                    compile_options,
                )
                .map_err(|err| format!("failed to compile native application artifact: {err}"))?;
                Ok(DaemonCompiledOutput::WrittenToPath)
            }
        }
    }
}
