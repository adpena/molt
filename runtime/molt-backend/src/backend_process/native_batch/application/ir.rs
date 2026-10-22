use molt_backend::SimpleIR;

use super::super::NativeApplicationArtifactOptions;

pub(crate) struct NativeApplicationStats {
    pub(crate) function_count: usize,
    total_ops: usize,
}

impl NativeApplicationStats {
    pub(crate) fn fits_single_batch(&self, batch_size: usize, batch_ops_budget: usize) -> bool {
        self.function_count <= batch_size && self.total_ops <= batch_ops_budget
    }
}

pub(crate) fn prepare_native_application_ir(
    ir: &mut SimpleIR,
    options: &NativeApplicationArtifactOptions<'_>,
) -> std::io::Result<()> {
    // One admission and application sequence for direct backend runs, daemon
    // requests and the development JIT runner.
    molt_backend::prepare_native_application_ir(
        ir,
        options.module_registry.as_ref(),
        &options.code_slots,
        options.stdlib_split_enabled,
    )
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))
}

pub(crate) fn native_application_stats(ir: &SimpleIR) -> NativeApplicationStats {
    let (function_count, total_ops) = ir
        .functions
        .iter()
        .filter(|func| !func.is_extern)
        .fold((0usize, 0usize), |(count, ops), func| {
            (count + 1, ops.saturating_add(func.ops.len()))
        });
    NativeApplicationStats {
        function_count,
        total_ops,
    }
}
