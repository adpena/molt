// Windows bin-test builds compile Unix daemon protocol code for parser coverage
// without running the daemon loop; production warning policy remains unchanged.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use molt_backend::SimpleIR;
#[cfg(feature = "luau-backend")]
use molt_backend::luau::LuauBackend;
#[cfg(feature = "rust-backend")]
use molt_backend::rust::RustBackend;
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};
use molt_tir::ir_rewrites::rewrite_annotate_stubs;
use std::env;
use std::io::Write;
use std::io::{self, Read};
use std::path::Path;
use std::time::Instant;

mod backend_process;
mod fact_graph_emit;
use backend_process::*;
use fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LuauTirModulePipelineStats {
    functions: usize,
    module_changed: usize,
}

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

fn validate_fact_graph_cli_contract(
    output_path: Option<&str>,
    function_name: Option<&str>,
    is_rust: bool,
) -> io::Result<()> {
    if output_path.is_some() != function_name.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fact-graph-output and --fact-graph-function must be supplied together",
        ));
    }
    if output_path.is_some() && is_rust {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fact graph emission does not support the rust target",
        ));
    }
    Ok(())
}

#[allow(clippy::vec_init_then_push)] // pushes are behind #[cfg] feature gates
fn main() -> io::Result<()> {
    // TIR optimization is mandatory. Invalid roundtrips are fatal compiler
    // bugs and must be debugged through dumps/verifier evidence, not by
    // bypassing typed IR.

    // Hard memory guard: set rlimit on virtual memory to prevent OOM
    // from crashing the entire machine. The default scales with host memory
    // so large TIR-enabled stdlib builds do not trip an artificially tiny cap.
    #[cfg(unix)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            let rlim = libc::rlimit {
                rlim_cur: max_bytes,
                rlim_max: max_bytes,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                // Silently ignore on macOS (Apple Silicon). MOLT_DEBUG_RLIMIT=1 to warn.
                if std::env::var("MOLT_DEBUG_RLIMIT").as_deref() == Ok("1") {
                    eprintln!(
                        "WARNING: failed to set memory limit (RLIMIT_AS={max_gb}GB). OOM guard not active."
                    );
                }
            }
        }
    }

    // Windows memory guard: use job objects to limit working set.
    // Less effective than Unix RLIMIT_AS but prevents unbounded growth.
    #[cfg(windows)]
    {
        let max_gb: u64 = std::env::var("MOLT_BACKEND_MAX_RSS_GB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_backend_max_rss_gb);
        let max_bytes = max_gb * 1024 * 1024 * 1024;
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            use windows_sys::Win32::System::Threading::*;
            let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = core::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = max_bytes as usize;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                AssignProcessToJobObject(job, GetCurrentProcess());
            }
        }
    }

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--features") {
        let features: &[&str] = &[
            #[cfg(feature = "native-backend")]
            "native-backend",
            #[cfg(feature = "luau-backend")]
            "luau-backend",
            #[cfg(feature = "wasm-backend")]
            "wasm-backend",
            #[cfg(feature = "rust-backend")]
            "rust-backend",
            #[cfg(feature = "cbor")]
            "cbor",
        ];
        if features.is_empty() {
            println!("molt-backend: no features enabled");
        } else {
            println!("molt-backend features: {}", features.join(", "));
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--daemon") {
        let socket_path = args
            .iter()
            .position(|arg| arg == "--socket")
            .and_then(|idx| args.get(idx + 1))
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))?;
        return run_daemon(socket_path);
    }
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());
    let is_rust = args.contains(&"--target".to_string()) && args.contains(&"rust".to_string());
    let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());
    #[allow(unused_variables)]
    let use_ir_pipeline = args.contains(&"--ir-pipeline".to_string());
    #[cfg_attr(not(feature = "native-backend"), allow(unused_variables))]
    let target_triple = args
        .iter()
        .position(|arg| arg == "--target-triple")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let output_path = args
        .iter()
        .position(|arg| arg == "--output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    let native_batch_job_file = args
        .iter()
        .position(|arg| arg == "--native-batch-job-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    #[cfg(feature = "native-backend")]
    if let Some(job_file) = native_batch_job_file {
        let output_file = output_path.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--output is required with --native-batch-job-file",
            )
        })?;
        compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_file))?;
        return Ok(());
    }

    let ir_file_path = args
        .iter()
        .position(|arg| arg == "--ir-file")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_output_path = args
        .iter()
        .position(|arg| arg == "--fact-graph-output")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    let fact_graph_function = args
        .iter()
        .position(|arg| arg == "--fact-graph-function")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str);
    validate_fact_graph_cli_contract(fact_graph_output_path, fact_graph_function, is_rust)?;

    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_link_flag = args.iter().any(|arg| arg == "--wasm-link");
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_data_base = args
        .iter()
        .position(|arg| arg == "--wasm-data-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_table_base = args
        .iter()
        .position(|arg| arg == "--wasm-table-base")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());
    #[cfg_attr(not(feature = "wasm-backend"), allow(unused_variables))]
    let wasm_split_runtime_runtime_table_min = args
        .iter()
        .position(|arg| arg == "--wasm-split-runtime-runtime-table-min")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|raw| raw.parse::<u32>().ok());

    let ir_format = args
        .iter()
        .position(|arg| arg == "--ir-format")
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str)
        .unwrap_or("json");

    // Read and parse IR.  Drop the raw buffer immediately after
    // deserialization to avoid holding two copies in memory simultaneously.
    let stdin_request_limit_bytes = stdin_request_limit_bytes();
    let document: molt_backend::BackendIrDocument = {
        if ir_format == "msgpack" {
            // msgpack binary format — deserialize directly via serde
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                // Streaming msgpack from stdin via BufReader — avoids
                // loading the entire IR into a Vec<u8> first.
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::with_capacity(1 << 20, bounded);
                match rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid msgpack IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if ir_format == "cbor" {
            // CBOR binary format — deserialize via ciborium
            #[cfg(not(feature = "cbor"))]
            {
                eprintln!("CBOR support requires the 'cbor' feature");
                std::process::exit(1);
            }
            #[cfg(feature = "cbor")]
            {
                if let Some(ir_path) = ir_file_path {
                    let file = std::fs::File::open(ir_path).map_err(|e| {
                        io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                    })?;
                    let reader = io::BufReader::new(file);
                    match ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(reader) {
                        Ok(ir) => ir,
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    let buf = read_bounded_request_bytes(
                        io::stdin().lock(),
                        stdin_request_limit_bytes,
                        "backend stdin request",
                    )?;
                    match ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(&buf[..])
                    {
                        Ok(ir) => {
                            drop(buf);
                            ir
                        }
                        Err(err) => {
                            eprintln!("invalid CBOR IR: {err}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        } else if ir_format == "ndjson" {
            // NDJSON streaming format — one function per line
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            } else {
                let stdin = io::stdin();
                let bounded = RequestBoundedRead::new(
                    stdin.lock(),
                    stdin_request_limit_bytes,
                    "backend stdin request",
                );
                let reader = io::BufReader::new(bounded);
                match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
                    Ok(ir) => ir,
                    Err(err) => {
                        eprintln!("invalid NDJSON IR: {err}");
                        std::process::exit(1);
                    }
                }
            }
        } else if let Some(ir_path) = ir_file_path {
            // Stream JSON directly from file — never holds raw JSON string in memory.
            let file = std::fs::File::open(ir_path).map_err(|e| {
                io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
            })?;
            let reader = io::BufReader::with_capacity(1 << 20, file);
            match serde_json::from_reader::<_, molt_backend::BackendIrDocument>(reader) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            // Stdin: read into string then deserialize directly (skips DOM intermediate).
            let raw_bytes = read_bounded_request_bytes(
                io::stdin().lock(),
                stdin_request_limit_bytes,
                "backend stdin request",
            )?;
            let buffer = String::from_utf8(raw_bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("backend stdin request is not UTF-8: {err}"),
                )
            })?;
            let result = serde_json::from_str::<molt_backend::BackendIrDocument>(&buffer);
            drop(buffer);
            match result {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("invalid IR JSON: {err}");
                    std::process::exit(1);
                }
            }
        }
    };
    let molt_backend::BackendIrDocument {
        mut ir,
        module_registry,
    } = document;
    // The registry projection belongs to the native application-object lane;
    // other lanes must not silently drop it.
    if module_registry.is_some() && (is_wasm || is_luau || is_rust) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "module_registry is a native-lane IR section; wasm/luau/rust lanes do not consume it",
        ));
    }

    rewrite_annotate_stubs(&mut ir);

    // Source emitters do not link against native/WASM runtime objects, so they
    // prune unreachable runtime/bootstrap support before textual codegen.
    if is_luau {
        ir.tree_shake_luau();
    }

    if let (Some(path), Some(function_name)) = (fact_graph_output_path, fact_graph_function) {
        let backend_setting = std::env::var("MOLT_BACKEND").ok();
        let target_info = if is_luau {
            molt_backend::tir::target_info::TargetInfo::luau_release_fast()
        } else if is_wasm {
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
    if is_luau {
        let tir_start = Instant::now();
        let module_stats = run_luau_tir_module_pipeline(&mut ir)?;
        let tir_elapsed = tir_start.elapsed();
        eprintln!(
            "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
            module_stats.functions, module_stats.module_changed
        );
        molt_backend::eliminate_dead_ops(&mut ir);
    }

    let output_kind = if is_luau {
        BackendOutputKind::Luau
    } else if is_rust {
        BackendOutputKind::Rust
    } else if is_wasm {
        BackendOutputKind::Wasm
    } else {
        BackendOutputKind::Native
    };
    let output_file = resolve_backend_output_path(output_path, output_kind);
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to create backend output parent for '{}': {}",
                output_file, err
            ),
        )
    })?;
    if is_luau {
        #[cfg(feature = "luau-backend")]
        {
            let mut backend = LuauBackend::new();
            let source = if use_ir_pipeline {
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
    } else if is_rust {
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
    } else if is_wasm {
        #[cfg(feature = "wasm-backend")]
        {
            let mut file = create_backend_output_file(output_file).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to create backend output '{}': {}", output_file, err),
                )
            })?;
            let mut options = WasmCompileOptions::default();
            if wasm_link_flag {
                options.reloc_enabled = true;
            }
            if let Some(value) = wasm_data_base {
                options.data_base = value;
            }
            if let Some(value) = wasm_table_base {
                options.table_base = value;
            }
            if let Some(value) = wasm_split_runtime_runtime_table_min {
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
            // ── Incremental compilation ──
            // When MOLT_STDLIB_OBJ is set to a path, the backend caches
            // stdlib compilation: stdlib functions compile once to that path,
            // subsequent builds skip them entirely.  User functions always
            // recompile.  This reduces builds from ~5min to ~3sec.
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

#[cfg(test)]
#[path = "main_tests/mod.rs"]
mod tests;
