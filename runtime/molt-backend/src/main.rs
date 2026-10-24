// Tests save, set and restore process settings around fixtures; production
// code reads them through `molt_ir::backend_environment` (clippy.toml).
#![cfg_attr(test, allow(clippy::disallowed_methods))]

#[cfg(not(any(
    feature = "native-backend",
    feature = "wasm-backend",
    feature = "luau-backend",
    feature = "rust-backend"
)))]
compile_error!(
    "molt-backend needs a backend: enable native-backend, wasm-backend, luau-backend or rust-backend"
);

use std::env;
use std::io;
use std::path::Path;

mod backend_process;
mod fact_graph_emit;
mod native_artifact_facts;
use backend_process::*;
use fact_graph_emit::{FactGraphEmitRequest, emit_fact_graph_for_ir};

fn main() -> io::Result<()> {
    // TIR optimization is mandatory. Invalid roundtrips are fatal compiler
    // bugs and must be debugged through dumps/verifier evidence, not by
    // bypassing typed IR.
    install_process_memory_guard();

    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("--scan-native-artifact-facts") {
        return native_artifact_facts::emit_cli(&args[2..]);
    }

    if args.get(1).map(String::as_str) == Some("--native-codegen-identity") {
        #[cfg(feature = "native-backend")]
        {
            if args.len() != 3 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--native-codegen-identity requires exactly one target triple",
                ));
            }
            let identity = molt_backend::SimpleBackend::object_codegen_identity(Some(&args[2]));
            serde_json::to_writer(
                io::stdout().lock(),
                &serde_json::json!({
                    "schema": "molt.native-codegen-identity.v1",
                    "requested_target": args[2],
                    "identity": identity,
                }),
            )?;
            println!();
            return Ok(());
        }
        #[cfg(not(feature = "native-backend"))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "molt-backend was built without native codegen identity support",
        ));
    }
    match BackendCommand::parse(args.get(1..).unwrap_or_default())? {
        BackendCommand::Features => {
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
            println!("molt-backend features: {}", features.join(", "));
            Ok(())
        }
        #[cfg(molt_backend_daemon)]
        BackendCommand::Daemon {
            socket_path,
            emission_scope,
        } => {
            backend_process::origin::initialize_scope(emission_scope)?;
            run_daemon(socket_path)
        }
        #[cfg(feature = "native-backend")]
        BackendCommand::NativeBatchJob {
            job_file,
            output_path,
            emission_scope,
        } => {
            backend_process::origin::initialize_scope(emission_scope)?;
            let origin =
                compile_native_batch_object_job_file(Path::new(job_file), Path::new(output_path))?;
            serde_json::to_writer(io::stdout().lock(), &origin).map_err(io::Error::other)
        }
        BackendCommand::Compile(request) => compile_ir_document(request),
    }
}

fn compile_ir_document(request: BackendCompileArgs<'_>) -> io::Result<()> {
    let fact_graph = request
        .fact_graph
        .map(|fact_graph| {
            let target_info = request.target.fact_graph_target_info()?;
            Ok::<_, io::Error>((fact_graph, target_info))
        })
        .transpose()?;
    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    backend_process::origin::initialize_scope(request.emission_scope)?;
    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    let (document, captured_input) = if request.capture_origin.is_some() {
        let (document, input) = read_captured_msgpack(request.ir_file_path)?;
        (document, Some(input))
    } else {
        (
            read_backend_ir_document(request.ir_format, request.ir_file_path)?,
            None,
        )
    };
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    let document = read_backend_ir_document(request.ir_format, request.ir_file_path)?;
    let document = request.target.admit_document(document)?;

    if let Some((fact_graph, target_info)) = fact_graph {
        emit_fact_graph_for_ir(
            &document.ir,
            FactGraphEmitRequest {
                output_path: Path::new(fact_graph.output_path),
                function_name: fact_graph.function_name,
                target_info: &target_info,
            },
        )?;
        eprintln!(
            "Wrote TIR fact graph for '{}' to {}",
            fact_graph.function_name, fact_graph.output_path
        );
        return Ok(());
    }

    let emission = BackendTargetEmitRequest {
        document,
        output_path: request.output_path,
        target: request.target,
        capture_origin: request.capture_origin.is_some(),
    };
    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    let emitted = emit_backend_target(emission)?;
    #[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
    emit_backend_target(emission)?;
    #[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
    if let Some(path) = request.capture_origin {
        let emitted =
            emitted.ok_or_else(|| io::Error::other("backend did not produce requested origin"))?;
        let origin = backend_process::origin::EmissionOrigin::new(
            backend_process::origin::ProducerKind::OneShot,
            captured_input.expect("capture decoded its input"),
            emitted.output,
            emitted.codegen,
        );
        let bytes = serde_json::to_vec(&origin).map_err(io::Error::other)?;
        molt_artifact_publish::write_bytes_atomically(Path::new(path), &bytes)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "main_tests/mod.rs"]
mod tests;
