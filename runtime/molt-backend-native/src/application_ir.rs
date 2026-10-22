//! Whole-application IR preparation shared by every native code owner.
//!
//! An AOT application object and a JIT program start from the same lowered
//! application and must run the same program, so both prepare it here.

use crate::ir::{FunctionIR, ModuleRegistryIR, SimpleIR};
use molt_ir::code_slots::CodeSlotBindings;
use molt_ir::ir_schema::FunctionOpShapeDiagnostic;
use std::collections::BTreeSet;

/// Admit a native application, then prepare its IR before `SimpleBackend`
/// compiles it.
///
/// Admission validates the program's code slots and module slots against the
/// whole lowered program, before anything is pruned, so a program that names
/// an unbound slot is refused even where the slot sits in a function that
/// dead-function elimination would remove.
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
    code_slots: &CodeSlotBindings,
    stdlib_split: bool,
) -> Result<(), FunctionOpShapeDiagnostic> {
    code_slots.validate_functions(&ir.functions)?;
    molt_ir::ir::validate_module_slots(module_registry, &ir.functions)?;
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
    Ok(())
}

/// Keep the first definition of each function name.
fn deduplicate_functions_by_name(functions: &mut Vec<FunctionIR>) {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    functions.retain(|function| seen.insert(function.name.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::OpIR;

    fn function(name: &str, ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            ops,
            ..FunctionIR::default()
        }
    }

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    /// A program whose only unbound code slot sits in a function nothing
    /// reaches. Dead-function elimination would remove it, so an admission
    /// that ran after pruning would accept the program.
    fn program_with_an_unreached_unbound_slot() -> SimpleIR {
        let mut slot = op("code_slot_set");
        slot.s_value = Some("never_bound".to_string());
        SimpleIR {
            functions: vec![
                function("molt_main", vec![op("ret_void")]),
                function("unreached", vec![slot, op("ret_void")]),
            ],
            profile: None,
        }
    }

    #[test]
    fn admission_sees_the_program_before_pruning() {
        let mut ir = program_with_an_unreached_unbound_slot();
        let diagnostic =
            prepare_native_application_ir(&mut ir, None, &CodeSlotBindings::default(), false)
                .expect_err("an unbound code slot is refused");
        assert_eq!(diagnostic.function, "unreached");
        // Nothing was pruned: the refusal left the program as it was.
        assert_eq!(ir.functions.len(), 2);
    }
}
