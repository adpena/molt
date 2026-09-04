use crate::tir::op_kinds_generated::{
    SIMPLEIR_RUNTIME_REQUIREMENT_DESCRIPTORS, SimpleIrRuntimeRequirements,
    simpleir_runtime_requirements_table, simpleir_runtime_symbol_requirements_table,
};
use crate::tir::target_info::TargetInfo;
use crate::{OpIR, SimpleIR};

pub use crate::tir::op_kinds_generated::{
    ASYNC_RUNTIME_REQUIREMENT_REASON, PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON,
};

pub fn validate_runtime_target_contract(
    ir: &SimpleIR,
    target_info: &TargetInfo,
) -> Result<(), String> {
    let target = target_info.target.as_str();
    let supported_requirements = target_info.supported_runtime_semantics;
    for function in &ir.functions {
        for (index, op) in function.ops.iter().enumerate() {
            let Some(requirements) = simpleir_op_runtime_requirements(op) else {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: operation is unclassified in the generated runtime semantic authority",
                    function.name, op.kind,
                ));
            };
            let missing = requirements.difference(supported_requirements);
            for descriptor in SIMPLEIR_RUNTIME_REQUIREMENT_DESCRIPTORS {
                if missing.contains(descriptor.requirement) {
                    return Err(format!(
                        "{target} target rejected before source generation: {}:op#{index} `{}`: {}",
                        function.name, op.kind, descriptor.reason,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Compose an op's generated semantic kind with canonical callable metadata.
/// Provenance is rejected on the acquisition op itself; backend target
/// admission deliberately performs no use-sensitive heap/CFG taint analysis.
pub fn simpleir_op_runtime_requirements(op: &OpIR) -> Option<SimpleIrRuntimeRequirements> {
    let mut requirements = simpleir_runtime_requirements_table(op.kind.as_str())?;
    if op.async_work_poll {
        requirements = requirements.union(SimpleIrRuntimeRequirements::PENDING_CALL_EVAL_BREAKER);
    }
    requirements = requirements.union(SimpleIrRuntimeRequirements::from_bits(
        op.runtime_requirement_bits,
    )?);
    if let Some(symbol) = op.runtime_symbol.as_deref() {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    if let Some(symbol) = op.builtin_name.as_deref() {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    if matches!(op.kind.as_str(), "call_internal" | "builtin_func")
        && let Some(symbol) = op.s_value.as_deref()
    {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    Some(requirements)
}
