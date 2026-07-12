use molt_backend::SimpleIR;
use molt_backend::{WasmBackend, WasmCompileOptions};
use std::io;
use std::io::Write;

use super::super::cli_args::WasmCliOptions;
use super::super::io_limits::create_backend_output_file;

pub(super) fn emit_wasm_target(
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
