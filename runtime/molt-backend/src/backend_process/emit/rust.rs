use molt_backend::SimpleIR;
use molt_backend::rust::RustBackend;
use std::io;
use std::path::Path;

use crate::backend_process::write_text_atomically;

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
    let source = rust_source_for_ir(ir)?;
    write_text_atomically(Path::new(output_file), &source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to publish backend output {output_file:?}: {error}"),
        )
    })?;
    println!("Successfully transpiled to {output_file}");
    Ok(())
}
