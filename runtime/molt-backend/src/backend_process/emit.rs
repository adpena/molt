use molt_backend::SimpleIR;
#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};
use std::io;
#[cfg(any(
    feature = "luau-backend",
    feature = "rust-backend",
    feature = "wasm-backend"
))]
use std::io::Write;
#[cfg(feature = "native-backend")]
use std::path::Path;
#[cfg(feature = "luau-backend")]
use std::time::Instant;

use super::cli_args::WasmCliOptions;
#[cfg(any(
    feature = "luau-backend",
    feature = "rust-backend",
    feature = "wasm-backend"
))]
use super::io_limits::create_backend_output_file;
use super::io_limits::{BackendOutputKind, ensure_output_parent_dir, resolve_backend_output_path};
#[cfg(feature = "native-backend")]
use super::native_batch::compile_native_application_object_to_path;
#[cfg(feature = "native-backend")]
use super::shared_stdlib_cache::{NativeStdlibCachePrepare, prepare_native_application_object};

pub(crate) struct BackendTargetEmitRequest<'a> {
    pub(crate) ir: SimpleIR,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
    pub(crate) output_path: Option<&'a str>,
    pub(crate) output_kind: BackendOutputKind,
    #[cfg_attr(not(feature = "luau-backend"), allow(dead_code))]
    pub(crate) use_ir_pipeline: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<&'a str>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_options: WasmCliOptions,
}

#[cfg(feature = "rust-backend")]
fn rust_source_for_ir(ir: &SimpleIR) -> io::Result<String> {
    let mut ir = ir.clone();
    ir.tree_shake_source_emission();
    let mut backend = RustBackend::new();
    backend.compile_checked(&ir).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Rust validation failed: {err}"),
        )
    })
}

#[cfg(feature = "luau-backend")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LuauTirModulePipelineStats {
    functions: usize,
    module_changed: usize,
}

#[cfg(feature = "luau-backend")]
fn run_luau_tir_module_pipeline(ir: &mut SimpleIR) -> io::Result<LuauTirModulePipelineStats> {
    let target_info = molt_backend::tir::target_info::TargetInfo::luau_release_fast();
    let local_function_count = ir.functions.iter().filter(|func| !func.is_extern).count();
    let mut tir_run = molt_backend::tir::pipeline_cache::run_cached_tir_pipeline(
        &mut ir.functions,
        molt_backend::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: molt_backend::tir::pipeline_cache::TirPipelineCacheFlavor::Luau,
            cache_dir: None,
            process_externs: false,
            verify_lir: false,
            tir_dump: std::env::var("TIR_DUMP").ok().as_deref() == Some("1"),
            tir_stats: std::env::var("TIR_OPT_STATS").ok().as_deref() == Some("1"),
            progress_prefix: None,
            resource_plan: molt_backend::tir::pipeline_cache::tir_optimization_resource_plan(),
        },
        |_| {},
    );
    let non_inlinable = std::collections::HashSet::new();
    let module_run =
        molt_backend::tir::pipeline_cache::run_simple_ir_module_pipeline_from_cached_tir(
            &mut ir.functions,
            &mut tir_run.cached_tir,
            molt_backend::tir::pipeline_cache::TirSimpleIrModulePipelineOptions {
                target_info: &target_info,
                module_name: "luau_module",
                non_inlinable: &non_inlinable,
                missing_tir_context: "Luau TIR cache runner",
                backconvert_context: "Luau TIR module pipeline",
                stage_observer: None,
            },
        );

    Ok(LuauTirModulePipelineStats {
        functions: local_function_count,
        module_changed: module_run.module_analysis.changed_functions.len(),
    })
}

pub(crate) fn emit_backend_target(request: BackendTargetEmitRequest<'_>) -> io::Result<()> {
    let output_file = resolve_backend_output_path(request.output_path, request.output_kind);
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;

    match request.output_kind {
        BackendOutputKind::Luau => {
            #[cfg(feature = "luau-backend")]
            {
                let mut ir = request.ir;
                emit_luau_target(&mut ir, output_file, request.use_ir_pipeline)?;
            }
            #[cfg(not(feature = "luau-backend"))]
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without luau-backend support; rebuild with: cargo build -p molt-backend --features luau-backend",
                ));
            }
        }
        BackendOutputKind::Rust => {
            #[cfg(feature = "rust-backend")]
            {
                emit_rust_target(&request.ir, output_file)?;
            }
            #[cfg(not(feature = "rust-backend"))]
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without rust-backend support; rebuild with: cargo build -p molt-backend --features rust-backend",
                ));
            }
        }
        BackendOutputKind::Wasm => {
            #[cfg(feature = "wasm-backend")]
            {
                emit_wasm_target(request.ir, output_file, request.wasm_options)?;
            }
            #[cfg(not(feature = "wasm-backend"))]
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend",
                ));
            }
        }
        BackendOutputKind::Native => {
            #[cfg(feature = "native-backend")]
            {
                emit_native_target(
                    request.ir,
                    request.module_registry,
                    output_file,
                    request.target_triple,
                )?;
            }
            #[cfg(not(feature = "native-backend"))]
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend",
                ));
            }
        }
    }

    Ok(())
}

#[cfg(feature = "luau-backend")]
fn emit_luau_target(ir: &mut SimpleIR, output_file: &str, use_ir_pipeline: bool) -> io::Result<()> {
    let tir_start = Instant::now();
    let module_stats = run_luau_tir_module_pipeline(ir)?;
    let tir_elapsed = tir_start.elapsed();
    eprintln!(
        "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
        module_stats.functions, module_stats.module_changed
    );
    molt_backend::eliminate_dead_ops(ir);

    let mut backend = LuauBackend::new();
    let source = if use_ir_pipeline {
        backend.compile_via_ir(ir)
    } else {
        backend.compile_checked(ir)
    }
    .map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Luau validation failed for '{}': {}", output_file, err),
        )
    })?;
    let mut file = create_backend_output_file(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create backend output '{}': {}", output_file, err),
        )
    })?;
    file.write_all(source.as_bytes())?;
    let lines = source.lines().count();
    eprintln!(
        "Successfully transpiled to {output_file} ({lines} lines, {:.1} KB)",
        source.len() as f64 / 1024.0
    );
    Ok(())
}

#[cfg(feature = "rust-backend")]
fn emit_rust_target(ir: &SimpleIR, output_file: &str) -> io::Result<()> {
    let mut file = create_backend_output_file(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create backend output '{}': {}", output_file, err),
        )
    })?;
    let source = rust_source_for_ir(ir)?;
    file.write_all(source.as_bytes())?;
    println!("Successfully transpiled to {output_file}");
    Ok(())
}

#[cfg(feature = "wasm-backend")]
fn emit_wasm_target(
    ir: SimpleIR,
    output_file: &str,
    wasm_options: WasmCliOptions,
) -> io::Result<()> {
    let mut file = create_backend_output_file(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create backend output '{}': {}", output_file, err),
        )
    })?;
    let mut options = WasmCompileOptions::default();
    if wasm_options.link_relocs {
        options.reloc_enabled = true;
    }
    if let Some(value) = wasm_options.data_base {
        options.data_base = value;
    }
    if let Some(value) = wasm_options.table_base {
        options.table_base = value;
    }
    if let Some(value) = wasm_options.split_runtime_runtime_table_min {
        options.split_runtime_runtime_table_min = Some(value);
    }
    let backend = WasmBackend::with_options(options);
    let wasm_bytes = backend.compile(ir);
    file.write_all(&wasm_bytes)?;
    println!("Successfully compiled to {output_file}");
    Ok(())
}

#[cfg(feature = "native-backend")]
fn emit_native_target(
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
