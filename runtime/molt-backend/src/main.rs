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

#[allow(clippy::vec_init_then_push)] // pushes are behind #[cfg] feature gates
fn main() -> io::Result<()> {
    // TIR optimization is mandatory. Invalid roundtrips are fatal compiler
    // bugs and must be debugged through dumps/verifier evidence, not by
    // bypassing typed IR.
    install_process_memory_guard();

    let args: Vec<String> = env::args().collect();
    let cli_args = BackendCliArgs::parse(&args);
    if cli_args.wants_features {
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
    if let Some(socket_path) = cli_args.daemon_socket_path()? {
        return run_daemon(socket_path);
    }
    let is_wasm = cli_args.is_wasm;
    let is_rust = cli_args.is_rust;
    let is_luau = cli_args.is_luau;
    #[cfg(feature = "native-backend")]
    if let Some(job_file) = cli_args.native_batch_job_file {
        let output_file = cli_args.output_path.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--output is required with --native-batch-job-file",
            )
        })?;
        compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_file))?;
        return Ok(());
    }

    cli_args.validate_fact_graph_contract()?;

    let document = read_backend_ir_document(cli_args.ir_format, cli_args.ir_file_path)?;
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

    if let (Some(path), Some(function_name)) = (
        cli_args.fact_graph_output_path,
        cli_args.fact_graph_function,
    ) {
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

    let output_kind = cli_args.output_kind();
    emit_backend_target(BackendTargetEmitRequest {
        ir,
        module_registry,
        output_path: cli_args.output_path,
        output_kind,
        use_ir_pipeline: cli_args.use_ir_pipeline,
        target_triple: cli_args.target_triple,
        wasm_options: cli_args.wasm_options,
    })?;

    Ok(())
}

#[cfg(test)]
#[path = "main_tests/mod.rs"]
mod tests;
