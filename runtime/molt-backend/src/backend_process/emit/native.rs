use molt_backend::SimpleIR;
use std::io;
use std::path::Path;

use super::super::native_batch::compile_native_application_object_to_path;
use super::super::shared_stdlib_cache::{
    NativeStdlibCachePrepare, prepare_native_application_object,
};

pub(super) fn emit_native_target(
    mut ir: SimpleIR,
    module_registry: Option<molt_backend::ModuleRegistryIR>,
    output_file: &str,
    target_triple: Option<&str>,
) -> io::Result<()> {
    let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
    let expected_stdlib_cache_key = std::env::var("MOLT_STDLIB_CACHE_KEY").ok();
    let expected_stdlib_cache_manifest = std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok();
    let have_entry_module = std::env::var("MOLT_ENTRY_MODULE").is_ok();
    let entry_module =
        std::env::var("MOLT_ENTRY_MODULE").unwrap_or_else(|_| "__main__".to_string());
    let explicit_stdlib_module_symbols = molt_backend::stdlib_module_symbols_from_env()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
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
            log_prefix: "MOLT_BACKEND",
            module_registry,
        },
    )?;

    compile_native_application_object_to_path(ir, Path::new(output_file), compile_options)?;
    Ok(())
}
