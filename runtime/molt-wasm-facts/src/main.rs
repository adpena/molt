use molt_host_process::host_process_environment_generated::HostProcessEnv;
use molt_host_process::{
    default_process_max_rss_gb, install_process_memory_limit, process_max_rss_bytes_from,
};
use std::io::{BufWriter, Read, Write};
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
    output: &mut impl Write,
    result: Result<molt_wasm_facts::WasmLinkFacts, String>,
) -> io::Result<bool> {
    let passed = match result {
        Ok(facts) => {
            serde_json::to_writer(
                &mut *output,
                &WasmLinkFactsSuccess {
                    schema_version: molt_wasm_facts::WASM_LINK_FACTS_SCHEMA_VERSION,
                    ok: true,
                    facts: &facts,
                },
            )?;
            true
        }
        Err(error) => {
            serde_json::to_writer(
                &mut *output,
                &WasmLinkFactsFailure {
                    schema_version: molt_wasm_facts::WASM_LINK_FACTS_SCHEMA_VERSION,
                    ok: false,
                    error: &error,
                },
            )?;
            false
        }
    };
    writeln!(output)?;
    Ok(passed)
}

/// Process one declared module at a time. A length never causes an allocation
/// until the corresponding bytes have actually arrived on stdin.
fn scan_wasm_link_facts_batch(
    input: &mut impl Read,
    output: &mut impl Write,
    lengths: &str,
) -> io::Result<bool> {
    let mut bytes = Vec::new();
    for length in lengths.split(',') {
        let Ok(size) = length.parse::<u64>() else {
            return emit_wasm_link_facts_result(
                output,
                Err("invalid wasm facts module length".into()),
            );
        };
        bytes.clear();
        let result = match (&mut *input).take(size).read_to_end(&mut bytes) {
            Ok(_) if bytes.len() as u64 == size => molt_wasm_facts::scan_wasm_link_facts(&bytes),
            Ok(_) => Err("truncated wasm facts module".into()),
            Err(error) => Err(format!("cannot read wasm facts module: {error}")),
        };
        if !emit_wasm_link_facts_result(output, result)? {
            return Ok(false);
        }
    }
    match input.read_exact(&mut [0]) {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(true),
        result => emit_wasm_link_facts_result(
            output,
            Err(match result {
                Ok(()) => "unexpected bytes after the declared wasm facts modules".into(),
                Err(error) => format!("cannot close wasm facts input: {error}"),
            }),
        ),
    }
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
    let mut output = BufWriter::new(io::stdout().lock());
    let passed = match args.as_slice() {
        [_, command, lengths] if command == "--scan-wasm-link-facts" => {
            scan_wasm_link_facts_batch(&mut io::stdin().lock(), &mut output, lengths)?
        }
        _ => emit_wasm_link_facts_result(
            &mut output,
            Err("expected --scan-wasm-link-facts followed by comma-separated byte lengths".into()),
        )?,
    };
    output.flush()?;
    if !passed {
        std::process::exit(2);
    }
    Ok(())
}
