use molt_backend::SimpleIR;
use molt_backend::rust::RustBackend;
use std::io;
use std::io::Write;

use super::super::io_limits::create_backend_output_file;

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

pub(super) fn emit_rust_target(ir: &SimpleIR, output_file: &str) -> io::Result<()> {
    let mut file = create_backend_output_file(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create backend output '{}': {}", output_file, err),
        )
    })?;
    let source = rust_source_for_ir(ir)?;
    file.write_all(source.as_bytes())?;
    println!("Successfully transpiled to {output_file}");
    Ok(())
}
