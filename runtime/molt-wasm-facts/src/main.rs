use molt_host_process::host_process_environment_generated::HostProcessEnv;
use molt_host_process::{
    default_process_max_rss_gb, install_process_memory_limit, process_max_rss_bytes_from,
};
use std::io::Read;
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

/// Read the whole module from stdin. The producer holds these bytes in memory
/// already, so a pipe replaces a scratch file: no module-sized write and read
/// per scan (HF-26).
fn read_wasm_facts_input() -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read wasm facts input from stdin: {error}"))?;
    Ok(bytes)
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
            let result = if args.len() == 2 {
                read_wasm_facts_input()
                    .and_then(|bytes| molt_wasm_facts::scan_wasm_link_facts(&bytes))
            } else {
                Err(
                    "--scan-wasm-link-facts reads the module from stdin and takes no arguments"
                        .to_string(),
                )
            };
            emit_wasm_link_facts_result(result)
        }
        _ => emit_wasm_link_facts_result(Err("expected --scan-wasm-link-facts".into())),
    }
}
