use crate::backend_process::*;
use crate::backend_request::BackendCliRequest;
use crate::fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};
use molt_backend::SimpleIR;
#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};
use molt_tir::ir_rewrites::rewrite_annotate_stubs;
use std::io;
use std::path::Path;
use std::time::Instant;

#[cfg(feature = "rust-backend")]
pub(crate) fn rust_source_for_ir(ir: &SimpleIR) -> io::Result<String> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LuauTirModulePipelineStats {
    pub(crate) functions: usize,
    pub(crate) module_changed: usize,
}

pub(crate) fn run_luau_tir_module_pipeline(
    ir: &mut SimpleIR,
) -> io::Result<LuauTirModulePipelineStats> {
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

pub(crate) fn emit_backend_output_for_request(
    request: &BackendCliRequest<'_>,
    document: molt_backend::BackendIrDocument,
) -> io::Result<()> {
    let molt_backend::BackendIrDocument {
        mut ir,
        module_registry,
    } = document;
    // The registry projection belongs to the native application-object lane;
    // other lanes must not silently drop it.
    if module_registry.is_some() && (request.is_wasm || request.is_luau || request.is_rust) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "module_registry is a native-lane IR section; wasm/luau/rust lanes do not consume it",
        ));
    }

    rewrite_annotate_stubs(&mut ir);

    // Source emitters do not link against native/WASM runtime objects, so they
    // prune unreachable runtime/bootstrap support before textual codegen.
    if request.is_luau {
        ir.tree_shake_luau();
    }

    if let (Some(path), Some(function_name)) =
        (request.fact_graph_output_path, request.fact_graph_function)
    {
        let backend_setting = std::env::var("MOLT_BACKEND").ok();
        let target_info = if request.is_luau {
            molt_backend::tir::target_info::TargetInfo::luau_release_fast()
        } else if request.is_wasm {
            molt_backend::tir::target_info::TargetInfo::wasm_release_fast()
        } else if backend_setting.as_deref() == Some("llvm") {
            molt_backend::tir::target_info::TargetInfo::llvm_release_fast()
        } else {
            molt_backend::tir::target_info::TargetInfo::native_release_fast()
        };
        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: Path::new(path),
                function_name,
                target_info: &target_info,
            },
        )?;
        eprintln!("Wrote TIR fact graph for '{function_name}' to {path}");
        return Ok(());
    }

    // Luau module phase (Tier-2 E1 parity): source emission is still one
    // compilation unit, so every local body is owned by this module and the
    // inliner has no external-linkage exclusions. Keep Luau on the same
    // structural path as native/WASM: lift once to TIR, run every local
    // function through the per-function pipeline, then run the whole-module
    // pipeline (E1 inliner, generator fusion, module-slot promotion, terminal
    // DropInsertion) before one fail-closed back-conversion.
    if request.is_luau {
        let tir_start = Instant::now();
        let module_stats = run_luau_tir_module_pipeline(&mut ir)?;
        let tir_elapsed = tir_start.elapsed();
        eprintln!(
            "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
            module_stats.functions, module_stats.module_changed
        );
        molt_backend::eliminate_dead_ops(&mut ir);
    }

    let output_file = resolve_backend_output_path(request.output_path, request.output_kind());
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;
    if request.is_luau {
        #[cfg(feature = "luau-backend")]
        {
            let mut backend = LuauBackend::new();
            let source = if request.use_ir_pipeline {
                backend.compile_via_ir(&ir)
            } else {
                backend.compile_checked(&ir)
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
        }
        #[cfg(not(feature = "luau-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without luau-backend support; rebuild with: cargo build -p molt-backend --features luau-backend",
            ));
        }
    } else if request.is_rust {
        #[cfg(feature = "rust-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let source = rust_source_for_ir(&ir)?;
            file.write_all(source.as_bytes())?;
            println!("Successfully transpiled to {output_file}");
        }
        #[cfg(not(feature = "rust-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without rust-backend support; rebuild with: cargo build -p molt-backend --features rust-backend",
            ));
        }
    } else if request.is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let mut options = WasmCompileOptions::default();
            if request.wasm_link_flag {
                options.reloc_enabled = true;
            }
            if let Some(value) = request.wasm_data_base {
                options.data_base = value;
            }
            if let Some(value) = request.wasm_table_base {
                options.table_base = value;
            }
            if let Some(value) = request.wasm_split_runtime_runtime_table_min {
                options.split_runtime_runtime_table_min = Some(value);
            }
            let backend = WasmBackend::with_options(options);
            let wasm_bytes = backend.compile(ir);
            file.write_all(&wasm_bytes)?;
            println!("Successfully compiled to {output_file}");
        }
        #[cfg(not(feature = "wasm-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without wasm-backend support; rebuild with: cargo build -p molt-backend --features wasm-backend",
            ));
        }
    } else {
        #[cfg(feature = "native-backend")]
        {
            // Incremental compilation:
            // MOLT_STDLIB_OBJ caches stdlib compilation. User functions always
            // recompile; this keeps one authority for the native object split.
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
                    target_triple: request.target_triple,
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
        }
        #[cfg(not(feature = "native-backend"))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backend binary was built without native-backend support; rebuild with: cargo build -p molt-backend --features native-backend",
            ));
        }
    }

    Ok(())
}
