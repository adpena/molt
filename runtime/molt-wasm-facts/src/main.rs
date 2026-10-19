use molt_artifact_publish::{AtomicFilePublication, open_regular_artifact};
use molt_host_process::host_process_environment_generated::HostProcessEnv;
use molt_host_process::{
    default_process_max_rss_gb, install_process_memory_limit, process_max_rss_bytes_from,
};
use std::path::Path;
use std::{env, io};

#[derive(serde::Serialize)]
struct WasmLinkFactsSuccess<'a> {
    schema_version: u32,
    ok: bool,
    facts: &'a molt_wasm_facts::WasmLinkFacts,
}

#[derive(serde::Serialize)]
struct WasmLinkFactsFailure<'a> {
    schema_version: u32,
    ok: bool,
    error: &'a str,
}

fn emit_wasm_link_facts_result(
    result: Result<molt_wasm_facts::WasmLinkFacts, String>,
) -> io::Result<()> {
    match result {
        Ok(facts) => {
            serde_json::to_writer(
                io::stdout().lock(),
                &WasmLinkFactsSuccess {
                    schema_version: molt_wasm_facts::WASM_LINK_FACTS_SCHEMA_VERSION,
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
                    schema_version: molt_wasm_facts::WASM_LINK_FACTS_SCHEMA_VERSION,
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

fn publish_wasm_link_facts_atomically(
    input: &Path,
    output: &Path,
    layout: Option<molt_wasm_facts::CallableTableLayout>,
    role: molt_wasm_facts::CallableTableArtifactRole,
) -> Result<molt_wasm_facts::WasmLinkFacts, String> {
    let file = open_regular_artifact(input)
        .map_err(|error| format!("cannot open wasm facts input {}: {error}", input.display()))?;
    let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }
        .map_err(|error| format!("cannot map wasm facts input {}: {error}", input.display()))?;
    let mut publication = AtomicFilePublication::new(output).map_err(|error| {
        format!(
            "cannot reserve atomic attested wasm output {}: {error}",
            output.display()
        )
    })?;
    let result = molt_wasm_facts::scan_and_write_callable_table_attestation(
        &bytes,
        layout,
        role,
        publication.writer(),
    );
    // Windows cannot replace a mapped input. End the read lease before the
    // single atomic commit so input == output is a supported, atomic update.
    drop(bytes);
    drop(file);
    let facts = match result {
        Ok(facts) => facts,
        Err(error) => return Err(publication.abort(io::Error::other(error)).to_string()),
    };
    publication.commit().map_err(|error| {
        format!(
            "cannot commit atomic attested wasm output {}: {error}",
            output.display()
        )
    })?;
    Ok(facts)
}

#[expect(
    clippy::disallowed_methods,
    reason = "standalone host tool reads the generated host-process environment catalog"
)]
fn install_memory_guard() {
    let raw = env::var(HostProcessEnv::MOLT_BACKEND_MAX_PROCESS_RSS_GB.name()).ok();
    let debug = env::var(HostProcessEnv::MOLT_DEBUG_RLIMIT.name()).as_deref() == Ok("1");
    install_process_memory_limit(
        process_max_rss_bytes_from(raw.as_deref(), default_process_max_rss_gb()),
        debug,
    );
}

fn main() -> io::Result<()> {
    install_memory_guard();
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--scan-wasm-link-facts") => {
            let result = args
                .get(2)
                .ok_or_else(|| "--scan-wasm-link-facts requires a wasm path".to_string())
                .and_then(|path| {
                    if args.len() != 3 {
                        return Err(
                            "--scan-wasm-link-facts accepts exactly one wasm path".to_string()
                        );
                    }
                    let file = open_regular_artifact(Path::new(path))
                        .map_err(|error| format!("cannot open wasm facts input {path}: {error}"))?;
                    // The linker hands this command an immutable finalized artifact and
                    // keeps the file descriptor alive for the scan. A read-only mapping
                    // avoids the 36-46 MiB whole-file heap copy paid by the former CLI.
                    let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }
                        .map_err(|error| format!("cannot map wasm facts input {path}: {error}"))?;
                    molt_wasm_facts::scan_wasm_link_facts(&bytes)
                });
            emit_wasm_link_facts_result(result)
        }
        Some("--publish-wasm-link-facts") => {
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
                publish_wasm_link_facts_atomically(
                    Path::new(input),
                    Path::new(output),
                    layout,
                    role,
                )
            })();
            emit_wasm_link_facts_result(result)
        }
        _ => emit_wasm_link_facts_result(Err(
            "expected --scan-wasm-link-facts or --publish-wasm-link-facts".into(),
        )),
    }
}
