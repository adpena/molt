use molt_backend::SimpleIR;
use molt_backend::luau::LuauBackend;
use std::io;
use std::path::Path;
use std::time::Instant;

use super::luau_pipeline::run_luau_tir_module_pipeline;
use crate::backend_process::atomic_publish::write_text_atomically;

pub(super) fn emit_luau_target(
    ir: &mut SimpleIR,
    output_file: &str,
    use_ir_pipeline: bool,
) -> io::Result<()> {
    let tir_start = Instant::now();
    let module_stats = run_luau_tir_module_pipeline(ir)?;
    let tir_elapsed = tir_start.elapsed();
    eprintln!(
        "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
        module_stats.functions, module_stats.module_changed
    );
    molt_backend::eliminate_dead_ops(ir);

    let mut backend = LuauBackend::new();
    let source = if use_ir_pipeline {
        backend.compile_via_ir(ir)
    } else {
        backend.compile_checked(ir)
    }
    .map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Luau validation failed for '{}': {}", output_file, err),
        )
    })?;
    write_text_atomically(Path::new(output_file), &source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to publish backend output {output_file:?}: {error}"),
        )
    })?;
    let lines = source.lines().count();
    eprintln!(
        "Successfully transpiled to {output_file} ({lines} lines, {:.1} KB)",
        source.len() as f64 / 1024.0
    );
    Ok(())
}
