//! Shared pre-source target admission for semantic domains that a backend's
//! value model may not represent exactly.

use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::op_kinds_generated::{
    SimpleIrIntegerSemantics, SimpleIrRuntimeRequirements, simpleir_integer_semantics_table,
    simpleir_runtime_requirements_table,
};
use crate::{OpIR, SimpleIR};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NumericTargetCapabilities {
    pub arbitrary_precision_integers: bool,
    pub cpython_float_divmod: bool,
    pub cpython_power: bool,
}

impl NumericTargetCapabilities {
    /// A fixed-width/double target with no complex-number representation and no
    /// shared CPython-exact float divmod authority.
    pub const FIXED_WIDTH_FLOAT_ONLY: Self = Self {
        arbitrary_precision_integers: false,
        cpython_float_divmod: false,
        cpython_power: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeTargetCapabilities {
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

/// Reject unsupported numeric semantics before a backend assembles source.
///
/// Operation membership comes exclusively from the generated op-kind registry;
/// target policy supplies only capabilities. Representation facts may prove a
/// dynamic operation stays entirely in a non-integer domain.
pub fn validate_numeric_target_contract(
    ir: &SimpleIR,
    target: &str,
    capabilities: NumericTargetCapabilities,
) -> Result<(), String> {
    for function in &ir.functions {
        let plan = ScalarRepresentationPlan::for_function_ir(function);
        for (index, op) in function.ops.iter().enumerate() {
            let role = simpleir_integer_semantics_table(op.kind.as_str());
            if let Some(reason) = numeric_admission_failure(&plan, op, role, capabilities) {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: {reason}",
                    function.name, op.kind,
                ));
            }
        }
    }
    Ok(())
}

pub fn validate_runtime_target_contract(
    ir: &SimpleIR,
    target: &str,
    capabilities: RuntimeTargetCapabilities,
) -> Result<(), String> {
    for function in &ir.functions {
        for (index, op) in function.ops.iter().enumerate() {
            let Some(requirements) = simpleir_runtime_requirements_table(op.kind.as_str()) else {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: operation is unclassified in the generated runtime semantic authority",
                    function.name, op.kind,
                ));
            };
            for (requirement, supported, reason) in runtime_admission_checks(capabilities) {
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

fn runtime_admission_checks(
    capabilities: RuntimeTargetCapabilities,
) -> [(SimpleIrRuntimeRequirements, bool, &'static str); 13] {
    [
        (
            SimpleIrRuntimeRequirements::IDENTITY,
            capabilities.python_identity,
            "operation requires Python object identity rather than value equality",
        ),
        (
            SimpleIrRuntimeRequirements::TUPLE,
            capabilities.tuple_representation,
            "operation requires a tuple representation distinct from mutable lists",
        ),
        (
            SimpleIrRuntimeRequirements::EXCEPTION,
            capabilities.exception_model,
            "operation requires Python exception state, matching, and structured unwinding",
        ),
        (
            SimpleIrRuntimeRequirements::DETERMINISTIC_LIFETIME,
            capabilities.deterministic_lifetime,
            "operation requires deterministic Python lifetime/finalizer semantics",
        ),
        (
            SimpleIrRuntimeRequirements::FORMAT_PROTOCOL,
            capabilities.format_protocol,
            "operation requires the Python __format__ protocol and conversion semantics",
        ),
        (
            SimpleIrRuntimeRequirements::ITERABLE_PROTOCOL,
            capabilities.iterable_protocol,
            "operation requires the Python iterable/sequence protocol and exact errors",
        ),
        (
            SimpleIrRuntimeRequirements::OBJECT_MODEL,
            capabilities.object_model,
            "operation requires Python aliasing, cycles, None storage, hashing, and object protocols",
        ),
        (
            SimpleIrRuntimeRequirements::TRUTHINESS,
            capabilities.python_truthiness,
            "operation requires CPython truthiness across NaN and dynamic containers",
        ),
        (
            SimpleIrRuntimeRequirements::COMPARISON,
            capabilities.python_comparison,
            "operation requires CPython comparison dispatch, NaN behavior, and exact integers",
        ),
        (
            SimpleIrRuntimeRequirements::FALLIBLE_PROTOCOL,
            capabilities.structured_runtime_errors,
            "operation can raise and requires structured catchable Python exceptions",
        ),
        (
            SimpleIrRuntimeRequirements::ASYNC_RUNTIME,
            capabilities.async_runtime,
            "operation requires an exact async scheduler and suspension-state model",
        ),
        (
            SimpleIrRuntimeRequirements::UNSTRUCTURED_CONTROL,
            capabilities.unstructured_control_flow,
            "operation requires a target-proven unstructured control-flow lowering",
        ),
        (
            SimpleIrRuntimeRequirements::HOST_CAPABILITY,
            capabilities.host_capabilities,
            "operation requires a target host filesystem or foreign-function capability",
        ),
    ]
}

fn numeric_admission_failure(
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
    role: SimpleIrIntegerSemantics,
    capabilities: NumericTargetCapabilities,
) -> Option<&'static str> {
    use SimpleIrIntegerSemantics as Role;

    match role {
        Role::None => None,
        Role::IntegerOnly | Role::IntegerProducer => (!capabilities.arbitrary_precision_integers)
            .then_some(
            "Python integer semantics require the canonical arbitrary-precision value authority",
        ),
        Role::DynamicAdd => {
            if capabilities.arbitrary_precision_integers
                || operands_are_float(plan, op, 2)
                || operands_are_strings(plan, op, 2)
            {
                None
            } else {
                Some(
                    "operands do not prove a non-integer add domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicNumeric | Role::DynamicTrueDiv => {
            if capabilities.arbitrary_precision_integers || operands_are_float(plan, op, 2) {
                None
            } else {
                Some(
                    "operands do not prove a float-only domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicUnaryNumeric => {
            if capabilities.arbitrary_precision_integers || operands_are_float(plan, op, 1) {
                None
            } else {
                Some(
                    "operand does not prove a float-only domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicDivmod => {
            if operands_are_float(plan, op, 2) {
                (!capabilities.cpython_float_divmod).then_some(
                    "float // and % require a CPython-exact signed-zero and rounding authority",
                )
            } else if operands_are_integer(plan, op, 2) {
                (!capabilities.arbitrary_precision_integers).then_some(
                    "integer // and % require the canonical arbitrary-precision value authority",
                )
            } else if capabilities.arbitrary_precision_integers && capabilities.cpython_float_divmod
            {
                None
            } else {
                Some(
                    "dynamic // and % require both arbitrary-precision integers and CPython-exact float divmod",
                )
            }
        }
        Role::DynamicPower => {
            if operands_are_float(plan, op, 2) {
                (!capabilities.cpython_power).then_some(
                    "pow requires CPython negative-base, fractional-exponent, and complex-result semantics",
                )
            } else if operands_are_integer(plan, op, 2) {
                (!capabilities.arbitrary_precision_integers).then_some(
                    "integer pow requires the canonical arbitrary-precision value authority",
                )
            } else if capabilities.arbitrary_precision_integers && capabilities.cpython_power {
                None
            } else {
                Some(
                    "dynamic pow requires arbitrary-precision integers and CPython complex-result semantics",
                )
            }
        }
    }
}

fn operands(op: &OpIR, arity: usize) -> Option<&[String]> {
    let args = op.args.as_deref()?;
    (args.len() == arity).then_some(args)
}

fn operands_are_float(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity).is_some_and(|args| args.iter().all(|name| plan.name_is_float_scalar(name)))
}

fn operands_are_integer(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity)
        .is_some_and(|args| args.iter().all(|name| plan.name_is_integer_family(name)))
}

fn operands_are_strings(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity).is_some_and(|args| args.iter().all(|name| plan.name_is_str_scalar(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionIR;

    fn binary(kind: &str, ty: &str) -> SimpleIR {
        SimpleIR {
            functions: vec![FunctionIR {
                name: "f".to_string(),
                params: vec!["lhs".to_string(), "rhs".to_string()],
                ops: vec![OpIR {
                    kind: kind.to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                }],
                param_types: Some(vec![ty.to_string(), ty.to_string()]),
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        }
    }

    #[test]
    fn fixed_width_targets_admit_exact_float_basics_only() {
        validate_numeric_target_contract(
            &binary("add", "float"),
            "test",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect("float add is exact in the target policy");

        for kind in ["pow", "floor_div", "mod"] {
            let error = validate_numeric_target_contract(
                &binary(kind, "float"),
                "test",
                NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
            )
            .expect_err("non-exact float semantics must reject");
            assert!(error.contains("rejected before source generation"));
        }
    }

    #[test]
    fn fixed_width_targets_reject_integer_arithmetic() {
        let error = validate_numeric_target_contract(
            &binary("add", "int"),
            "test",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect_err("i64 is not Python integer semantics");
        assert!(error.contains("arbitrary-precision"));
    }
}
