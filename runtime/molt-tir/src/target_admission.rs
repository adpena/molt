//! Shared pre-source target admission for semantic domains that a backend's
//! value model may not represent exactly.

mod numeric;
mod runtime;

pub use numeric::{exact_integer_literal_value, validate_numeric_target_contract};
pub use runtime::{simpleir_op_runtime_requirements, validate_runtime_target_contract};

use crate::SimpleIR;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NumericTargetCapabilities {
    pub arbitrary_precision_integers: bool,
    /// Largest concrete integer magnitude the target carrier represents
    /// exactly. This admits only literal values proven in range; it does not
    /// authorize integer-producing operations that can overflow that range.
    pub exact_integer_literal_max_magnitude: Option<u128>,
    pub cpython_float_divmod: bool,
    pub cpython_power: bool,
}

impl NumericTargetCapabilities {
    pub const FIXED_WIDTH_FLOAT_ONLY: Self = Self {
        arbitrary_precision_integers: false,
        exact_integer_literal_max_magnitude: None,
        cpython_float_divmod: false,
        cpython_power: false,
    };

    /// Luau's sole numeric carrier is IEEE-754 binary64. Every integer through
    /// magnitude 2^53 is represented exactly, while general integer producers
    /// remain inadmissible without an arbitrary-precision value authority.
    pub const LUAU_EXACT_INTEGER_LITERALS: Self = Self {
        arbitrary_precision_integers: false,
        exact_integer_literal_max_magnitude: Some(1_u128 << 53),
        cpython_float_divmod: false,
        cpython_power: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeTargetCapabilities {
    pub execution_frame_state: bool,
    pub python_frame_introspection: bool,
    pub python_identity: bool,
    pub tuple_representation: bool,
    pub exception_model: bool,
    pub deterministic_lifetime: bool,
    pub format_protocol: bool,
    pub iterable_protocol: bool,
    pub object_model: bool,
    pub python_truthiness: bool,
    pub python_comparison: bool,
    pub structured_runtime_errors: bool,
    pub async_runtime: bool,
    pub unstructured_control_flow: bool,
    pub host_capabilities: bool,
}

impl RuntimeTargetCapabilities {
    pub const NONE: Self = Self {
        execution_frame_state: false,
        python_frame_introspection: false,
        python_identity: false,
        tuple_representation: false,
        exception_model: false,
        deterministic_lifetime: false,
        format_protocol: false,
        iterable_protocol: false,
        object_model: false,
        python_truthiness: false,
        python_comparison: false,
        structured_runtime_errors: false,
        async_runtime: false,
        unstructured_control_flow: false,
        host_capabilities: false,
    };
}

/// Validate transport shape and every generated semantic-role family before
/// any target source buffer is touched.
pub fn validate_target_contract(
    ir: &SimpleIR,
    target: &str,
    numeric: NumericTargetCapabilities,
    runtime: RuntimeTargetCapabilities,
) -> Result<(), String> {
    crate::validate_simple_ir(ir)
        .map_err(|error| format!("{target} SimpleIR validation failed: {error}"))?;
    validate_numeric_target_contract(ir, target, numeric)?;
    validate_runtime_target_contract(ir, target, runtime)
}

#[cfg(test)]
mod tests;
