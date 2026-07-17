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

#[cfg(feature = "wasm-backend")]
#[derive(serde::Serialize)]
struct WasmLinkFactsSuccess<'a> {
    schema_version: u32,
    ok: bool,
    facts: &'a molt_wasm_facts::WasmLinkFacts,
}

#[cfg(feature = "wasm-backend")]
#[derive(serde::Serialize)]
struct WasmLinkFactsFailure<'a> {
    schema_version: u32,
    ok: bool,
    error: &'a str,
}

#[cfg(feature = "wasm-backend")]
fn emit_wasm_link_facts_result(
    result: Result<molt_wasm_facts::WasmLinkFacts, String>,
) -> io::Result<()> {
    match result {
        Ok(facts) => {
            serde_json::to_writer(
                io::stdout().lock(),
                &WasmLinkFactsSuccess {
                    schema_version: 3,
                    ok: true,
                    facts: &facts,
                },
            )?;
            println!();
        }
        Err(error) => {
            serde_json::to_writer(
                io::stdout().lock(),
                &WasmLinkFactsFailure {
                    schema_version: 3,
                    ok: false,
                    error: &error,
                },
            )?;
            println!();
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(feature = "wasm-backend")]
fn parse_callable_table_layout(
    value: &str,
) -> Result<molt_wasm_facts::CallableTableLayout, String> {
    let values = value
        .split(',')
        .map(|part| {
            part.parse::<u32>()
                .map_err(|error| format!("invalid callable-table layout value {part:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let [
        fixed_prefix_base,
        fixed_prefix_len,
        finalized_app_base,
        app_entry_count,
    ] = values.as_slice()
    else {
        return Err("--callable-table-layout requires four comma-separated u32 values".to_string());
    };
    Ok(molt_wasm_facts::CallableTableLayout {
        fixed_prefix_base: *fixed_prefix_base,
        fixed_prefix_len: *fixed_prefix_len,
        finalized_app_base: *finalized_app_base,
        app_entry_count: *app_entry_count,
    })
}

#[cfg(feature = "wasm-backend")]
fn publish_wasm_atomically<T>(
    output: &Path,
    publish: impl FnOnce(&mut io::BufWriter<std::fs::File>) -> Result<T, String>,
) -> Result<T, String> {
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "attested wasm output has no file name: {}",
                output.display()
            )
        })?;
    let mut temporary = None;
    for nonce in 0..1024u32 {
        let path =
            output.with_file_name(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create atomic attested wasm output beside {}: {error}",
                    output.display()
                ));
            }
        }
    }
    let (temporary_path, file) = temporary.ok_or_else(|| {
        format!(
            "cannot reserve atomic attested wasm output beside {}",
            output.display()
        )
    })?;
    let result = (|| {
        let mut writer = io::BufWriter::new(file);
        let value = publish(&mut writer)?;
        use std::io::Write as _;
        writer.flush().map_err(|error| {
            format!(
                "cannot flush atomic attested wasm output {}: {error}",
                temporary_path.display()
            )
        })?;
        writer.get_ref().sync_all().map_err(|error| {
            format!(
                "cannot sync atomic attested wasm output {}: {error}",
                temporary_path.display()
            )
        })?;
        drop(writer);
        atomic_replace_wasm_file(&temporary_path, output)?;
        Ok(value)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(all(feature = "wasm-backend", not(windows)))]
fn atomic_replace_wasm_file(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| {
        format!(
            "cannot atomically publish attested wasm {} -> {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(all(feature = "wasm-backend", windows))]
fn atomic_replace_wasm_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(format!(
            "cannot atomically publish attested wasm: {}",
            io::Error::last_os_error()
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
    if args.get(1).map(String::as_str) == Some("--scan-wasm-link-facts") {
        #[cfg(feature = "wasm-backend")]
        {
            let result = args
                .get(2)
                .ok_or_else(|| "--scan-wasm-link-facts requires a wasm path".to_string())
                .and_then(|path| {
                    if args.len() != 3 {
                        return Err(
                            "--scan-wasm-link-facts accepts exactly one wasm path".to_string()
                        );
                    }
                    let file = std::fs::File::open(path)
                        .map_err(|error| format!("cannot open wasm facts input {path}: {error}"))?;
                    // The linker hands this command an immutable finalized artifact and
                    // keeps the file descriptor alive for the scan. A read-only mapping
                    // avoids the 36-46 MiB whole-file heap copy paid by the former CLI.
                    let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }
                        .map_err(|error| format!("cannot map wasm facts input {path}: {error}"))?;
                    molt_wasm_facts::scan_wasm_link_facts(&bytes)
                });
            emit_wasm_link_facts_result(result)?;
            return Ok(());
        }
        #[cfg(not(feature = "wasm-backend"))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "molt-backend was built without wasm-backend facts support",
        ));
    }
    if args.get(1).map(String::as_str) == Some("--publish-wasm-link-facts") {
        #[cfg(feature = "wasm-backend")]
        {
            let result = (|| {
                let input = args
                    .get(2)
                    .ok_or("--publish-wasm-link-facts requires an input wasm path")?;
                let mut output = None;
                let mut layout = None;
                let mut role = molt_wasm_facts::CallableTableArtifactRole::Monolithic;
                let mut index = 3usize;
                while index < args.len() {
                    match args[index].as_str() {
                        "--output" => {
                            index += 1;
                            output =
                                Some(args.get(index).ok_or("--output requires a path")?.as_str());
                        }
                        "--callable-table-layout" => {
                            index += 1;
                            layout = Some(parse_callable_table_layout(
                                args.get(index)
                                    .ok_or("--callable-table-layout requires a value")?,
                            )?);
                        }
                        "--callable-table-role" => {
                            index += 1;
                            role = match args
                                .get(index)
                                .ok_or("--callable-table-role requires a value")?
                                .as_str()
                            {
                                "monolithic" => {
                                    molt_wasm_facts::CallableTableArtifactRole::Monolithic
                                }
                                "app" => molt_wasm_facts::CallableTableArtifactRole::App,
                                "runtime" => molt_wasm_facts::CallableTableArtifactRole::Runtime,
                                value => {
                                    return Err(format!(
                                        "invalid callable-table role {value:?}; expected monolithic, app, or runtime"
                                    ));
                                }
                            };
                        }
                        option => {
                            return Err(format!("unknown wasm facts publication option {option}"));
                        }
                    }
                    index += 1;
                }
                let output = output.ok_or("--publish-wasm-link-facts requires --output")?;
                let file = std::fs::File::open(input)
                    .map_err(|error| format!("cannot open wasm facts input {input}: {error}"))?;
                let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }
                    .map_err(|error| format!("cannot map wasm facts input {input}: {error}"))?;
                let facts = publish_wasm_atomically(Path::new(output), |writer| {
                    molt_wasm_facts::scan_and_write_callable_table_attestation(
                        &bytes, layout, role, writer,
                    )
                })?;
                Ok(facts)
            })();
            emit_wasm_link_facts_result(result)?;
            return Ok(());
        }
        #[cfg(not(feature = "wasm-backend"))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "molt-backend was built without wasm-backend facts support",
        ));
    }
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
    // Native and WASM consume the catalog. Textual source emitters do not ship
    // the Molt runtime/catalog ABI and must not silently drop it.
    if module_registry.is_some() && (is_luau || is_rust) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "module_registry requires a native or WASM runtime-backed target",
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
