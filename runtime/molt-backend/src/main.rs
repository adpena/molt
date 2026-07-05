// Windows bin-test builds compile Unix daemon protocol code for parser coverage
// without running the daemon loop; production warning policy remains unchanged.
#![cfg_attr(all(test, windows), allow(dead_code))]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use molt_tir::ir_rewrites::rewrite_annotate_stubs;
use std::env;
use std::io;
use std::path::Path;

mod backend_process;
mod fact_graph_emit;
use backend_process::*;
use fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};

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
    install_process_memory_guard();

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

    let document = read_backend_ir_document(ir_format, ir_file_path)?;
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

    let output_kind = if is_luau {
        BackendOutputKind::Luau
    } else if is_rust {
        BackendOutputKind::Rust
    } else if is_wasm {
        BackendOutputKind::Wasm
    } else {
        BackendOutputKind::Native
    };
    emit_backend_target(BackendTargetEmitRequest {
        ir,
        module_registry,
        output_path,
        output_kind,
        use_ir_pipeline,
        target_triple,
        wasm_options: WasmCliOptions {
            link_relocs: wasm_link_flag,
            data_base: wasm_data_base,
            table_base: wasm_table_base,
            split_runtime_runtime_table_min: wasm_split_runtime_runtime_table_min,
        },
    })?;

    Ok(())
}

#[cfg(test)]
#[path = "main_tests/mod.rs"]
mod tests;
