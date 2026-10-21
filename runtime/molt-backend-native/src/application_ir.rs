//! Whole-application IR preparation shared by every native code owner.
//!
//! An AOT application object and a JIT program start from the same lowered
//! application and must run the same program, so both prepare it here.

use crate::ir::{FunctionIR, ModuleRegistryIR, SimpleIR};
use std::collections::BTreeSet;

/// Prepare a native application's IR before `SimpleBackend` compiles it.
///
/// When `stdlib_split` is false this IR is the whole program: reachability
/// from the declared roots decides what codegen sees. Module init bodies are
/// reachable only through the registry blob's MODULE_INIT_TABLE relocations
/// (import bedrock invariant I5), so the registry's init symbols are roots.
/// A split application keeps its shared-stdlib declarations for the
/// partition, which prunes them itself.
pub fn prepare_native_application_ir(
    ir: &mut SimpleIR,
    module_registry: Option<&ModuleRegistryIR>,
    stdlib_split: bool,
) {
    crate::passes::inject_runtime_exit(ir);
    if !stdlib_split {
        let module_registry_roots: BTreeSet<String> = module_registry
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        crate::passes::eliminate_dead_functions_with_roots(ir, &module_registry_roots);
        crate::passes::eliminate_dead_imports(ir);
        crate::passes::eliminate_dead_ops(
            ir,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
    }
    deduplicate_functions_by_name(&mut ir.functions);
}

/// Keep the first definition of each function name.
fn deduplicate_functions_by_name(functions: &mut Vec<FunctionIR>) {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    functions.retain(|function| seen.insert(function.name.clone()));
}
