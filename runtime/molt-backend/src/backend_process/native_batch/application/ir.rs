use molt_backend::SimpleIR;

use super::super::{NativeApplicationObjectOptions, deduplicate_functions_by_name};

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
    options: &NativeApplicationObjectOptions<'_>,
) {
    // Preserve the one-shot native application-object sequence as the single
    // authority for both direct backend runs and daemon requests.
    molt_backend::inject_runtime_exit(ir);
    if !options.stdlib_split_enabled {
        // Import bedrock: registry init symbols are DFE roots - init bodies
        // are reachable only through the registry blob's MODULE_INIT_TABLE
        // relocations (invariant I5).
        let module_registry_roots: std::collections::BTreeSet<String> = options
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        molt_backend::eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        molt_backend::eliminate_dead_imports(ir);
        molt_backend::eliminate_dead_ops(ir);
    }
    deduplicate_functions_by_name(&mut ir.functions);
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
