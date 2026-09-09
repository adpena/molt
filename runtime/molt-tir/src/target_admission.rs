//! Shared pre-source target admission for semantic domains that a backend's
//! value model may not represent exactly.

mod numeric;
mod runtime;

pub use crate::tir::target_info::NumericTargetCapabilities;
pub use numeric::{exact_integer_literal_value, validate_numeric_target_contract};
pub use runtime::{
    ASYNC_RUNTIME_REQUIREMENT_REASON, PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON,
    validate_runtime_target_contract,
};

use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::target_info::TargetInfo;
use crate::{FunctionIR, SimpleIR};

/// Validate transport shape and every generated semantic-role family before
/// any target source buffer is touched.
pub fn validate_target_contract(ir: &SimpleIR, target_info: &TargetInfo) -> Result<(), String> {
    validate_target_contract_with_representation_plan(ir, target_info, |_, _| Ok(()))
}

/// Validate a target contract while sharing each function's representation
/// plan with a backend-specific semantic check. Representation planning lowers
/// and optimizes the whole function, so composing checks here prevents source
/// backends from independently rebuilding the same target plan.
pub fn validate_target_contract_with_representation_plan<F>(
    ir: &SimpleIR,
    target_info: &TargetInfo,
    mut validate_backend_semantics: F,
) -> Result<(), String>
where
    F: FnMut(&FunctionIR, &ScalarRepresentationPlan) -> Result<(), String>,
{
    let target = target_info.target.as_str();
    crate::validate_simple_ir(ir)
        .map_err(|error| format!("{target} SimpleIR validation failed: {error}"))?;
    if !target_info.extern_function_linkage
        && let Some(function) = ir.functions.iter().find(|function| function.is_extern)
    {
        return Err(format!(
            "{target} backend cannot compile extern function `{}`: the {target} target has no extern provider/linkage ABI",
            function.name
        ));
    }
    for function in &ir.functions {
        let plan = ScalarRepresentationPlan::for_function_ir_for_target(function, target_info);
        validate_backend_semantics(function, &plan)?;
        numeric::validate_numeric_function_target_contract(
            function,
            target,
            target_info.supported_numeric_semantics,
            &plan,
        )?;
    }
    validate_runtime_target_contract(ir, target_info)
}

#[cfg(test)]
mod tests;
