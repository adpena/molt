use super::RuntimeTargetCapabilities;
use crate::tir::op_kinds_generated::{
    SimpleIrRuntimeRequirements, simpleir_runtime_requirements_table,
    simpleir_runtime_symbol_requirements_table,
};
use crate::{OpIR, SimpleIR};

pub fn validate_runtime_target_contract(
    ir: &SimpleIR,
    target: &str,
    capabilities: RuntimeTargetCapabilities,
) -> Result<(), String> {
    let admission_checks = runtime_admission_checks(capabilities);
    for function in &ir.functions {
        for (index, op) in function.ops.iter().enumerate() {
            let Some(requirements) = simpleir_op_runtime_requirements(op) else {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: operation is unclassified in the generated runtime semantic authority",
                    function.name, op.kind,
                ));
            };
            for &(requirement, supported, reason) in &admission_checks {
                if requirements.contains(requirement) && !supported {
                    return Err(format!(
                        "{target} target rejected before source generation: {}:op#{index} `{}`: {reason}",
                        function.name, op.kind,
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

fn runtime_admission_checks(
    capabilities: RuntimeTargetCapabilities,
) -> [(SimpleIrRuntimeRequirements, bool, &'static str); 15] {
    use SimpleIrRuntimeRequirements as Requirement;
    [
        (
            Requirement::EXECUTION_FRAME,
            capabilities.execution_frame_state,
            "operation requires internal execution-frame stack and source-location custody",
        ),
        (
            Requirement::FRAME_INTROSPECTION,
            capabilities.python_frame_introspection,
            "operation requires exact Python-visible frame objects, locals, globals, and tracing state",
        ),
        (
            Requirement::IDENTITY,
            capabilities.python_identity,
            "operation requires Python object identity rather than value equality",
        ),
        (
            Requirement::TUPLE,
            capabilities.tuple_representation,
            "operation requires a tuple representation distinct from mutable lists",
        ),
        (
            Requirement::EXCEPTION,
            capabilities.exception_model,
            "operation requires Python exception state, matching, and structured unwinding",
        ),
        (
            Requirement::DETERMINISTIC_LIFETIME,
            capabilities.deterministic_lifetime,
            "operation requires deterministic Python lifetime/finalizer semantics",
        ),
        (
            Requirement::FORMAT_PROTOCOL,
            capabilities.format_protocol,
            "operation requires the Python __format__ protocol and conversion semantics",
        ),
        (
            Requirement::ITERABLE_PROTOCOL,
            capabilities.iterable_protocol,
            "operation requires the Python iterable/sequence protocol and exact errors",
        ),
        (
            Requirement::OBJECT_MODEL,
            capabilities.object_model,
            "operation requires Python aliasing, cycles, None storage, hashing, and object protocols",
        ),
        (
            Requirement::TRUTHINESS,
            capabilities.python_truthiness,
            "operation requires CPython truthiness across NaN and dynamic containers",
        ),
        (
            Requirement::COMPARISON,
            capabilities.python_comparison,
            "operation requires CPython comparison dispatch, NaN behavior, and exact integers",
        ),
        (
            Requirement::FALLIBLE_PROTOCOL,
            capabilities.structured_runtime_errors,
            "operation can raise and requires structured catchable Python exceptions",
        ),
        (
            Requirement::ASYNC_RUNTIME,
            capabilities.async_runtime,
            "operation requires an exact async scheduler and suspension-state model",
        ),
        (
            Requirement::UNSTRUCTURED_CONTROL,
            capabilities.unstructured_control_flow,
            "operation requires a target-proven unstructured control-flow lowering",
        ),
        (
            Requirement::HOST_CAPABILITY,
            capabilities.host_capabilities,
            "operation requires a target host filesystem or foreign-function capability",
        ),
    ]
}
