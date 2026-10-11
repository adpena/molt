use molt_backend::SimpleIR;
use molt_backend::luau::LuauBackend;
use std::io;
use std::path::Path;
use std::time::Instant;

use super::luau_pipeline::run_luau_tir_module_pipeline;
use molt_artifact_publish::write_text_atomically;

pub(super) fn emit_luau_target(mut ir: SimpleIR, output_file: &str) -> io::Result<()> {
    let luau_target = molt_backend::tir::target_info::TargetInfo::luau_release_fast();
    molt_backend::run_simple_ir_pipeline(&mut ir, &luau_target);
    let tir_start = Instant::now();
    let module_stats = run_luau_tir_module_pipeline(&mut ir)?;
    let tir_elapsed = tir_start.elapsed();
    eprintln!(
        "[molt-luau] TIR module pipeline: {} functions, {} module-changed in {tir_elapsed:.2?}",
        module_stats.functions, module_stats.module_changed
    );
    molt_backend::eliminate_dead_ops(&mut ir, &luau_target);

    let source = LuauBackend::new().compile_checked(&ir).map_err(|err| {
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
