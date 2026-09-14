use crate::SimpleIR;
use crate::tir::op_kinds_generated::SIMPLEIR_RUNTIME_REQUIREMENT_DESCRIPTORS;
use crate::tir::target_info::TargetInfo;

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
            if op.kind == "stack_alloc" {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: {}",
                    function.name,
                    op.kind,
                    crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED,
                ));
            }
            let Some(requirements) = op.runtime_requirements() else {
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
