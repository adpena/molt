use std::path::Path;
#[cfg(feature = "wasm-backend")]
use std::sync::Arc;

#[cfg(feature = "wasm-backend")]
use crate::backend_process::emit::validate_wasm_module_catalog;
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};

#[cfg(feature = "native-backend")]
use super::super::super::native_batch::compile_native_application_object_to_path;
#[cfg(feature = "native-backend")]
use super::super::super::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_object,
};
use super::super::DaemonJobRequest;

pub(crate) enum DaemonCompiledOutput {
    #[cfg(feature = "wasm-backend")]
    Bytes(Arc<[u8]>),
    WrittenToPath,
}

pub(crate) fn compile_daemon_job_output(
    job: &DaemonJobRequest,
    document: molt_backend::BackendIrDocument,
) -> Result<DaemonCompiledOutput, String> {
    let molt_backend::BackendIrDocument {
        mut ir,
        module_registry,
    } = document;

    if job.is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            if let Some(registry) = module_registry.as_ref() {
                validate_wasm_module_catalog(&ir, registry).map_err(|err| err.to_string())?;
            }
            let mut options = WasmCompileOptions {
                reloc_enabled: job.wasm_link,
                ..WasmCompileOptions::default()
            };
            if let Some(data_base) = job.wasm_data_base {
                options.data_base = data_base;
            }
            if let Some(table_base) = job.wasm_table_base {
                options.table_base = table_base;
            }
            if let Some(split_runtime_runtime_table_min) = job.wasm_split_runtime_runtime_table_min
            {
                options.split_runtime_runtime_table_min = Some(split_runtime_runtime_table_min);
            }
            let backend = WasmBackend::with_options(options).with_module_registry(module_registry);
            Ok(DaemonCompiledOutput::Bytes(Arc::from(backend.compile(ir))))
        }
        #[cfg(not(feature = "wasm-backend"))]
        {
            Err("backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend".to_string())
        }
    } else {
        #[cfg(feature = "native-backend")]
        {
            let target_triple = job.target_triple.as_deref();
            let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
            let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
            let expected_stdlib_cache_manifest = std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
            let entry_module =
                std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
            let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
            let explicit_stdlib_module_symbols = molt_backend::stdlib_module_symbols_from_env()?;
            let compile_options = prepare_native_application_object(
                &mut ir,
                NativeStdlibCachePrepare {
                    target_triple,
                    stdlib_obj_path: stdlib_obj_path.as_deref(),
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

            compile_native_application_object_to_path(ir, Path::new(&job.output), compile_options)
                .map_err(|err| format!("failed to compile native application object: {err}"))?;
            Ok(DaemonCompiledOutput::WrittenToPath)
        }
        #[cfg(not(feature = "native-backend"))]
        {
            Err("backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend".to_string())
        }
    }
}
