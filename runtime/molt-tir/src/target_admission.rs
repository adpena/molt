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
    /// Largest concrete integer magnitude the target carrier represents
    /// exactly. This admits only literal values proven in range; it does not
    /// authorize integer-producing operations that can overflow that range.
    pub exact_integer_literal_max_magnitude: Option<u128>,
    pub cpython_float_divmod: bool,
    pub cpython_power: bool,
}

impl NumericTargetCapabilities {
    /// A fixed-width/double target with no complex-number representation and no
    /// shared CPython-exact float divmod authority.
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
    pub python_frame_state: bool,
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
        python_frame_state: false,
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
    let admission_checks = runtime_admission_checks(capabilities);
    for function in &ir.functions {
        for (index, op) in function.ops.iter().enumerate() {
            let Some(requirements) = simpleir_runtime_requirements_table(op.kind.as_str()) else {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: operation is unclassified in the generated runtime semantic authority",
                    function.name, op.kind,
                ));
            };
            if requirements.is_empty() {
                continue;
            }
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

fn runtime_admission_checks(
    capabilities: RuntimeTargetCapabilities,
) -> [(SimpleIrRuntimeRequirements, bool, &'static str); 14] {
    [
        (
            SimpleIrRuntimeRequirements::FRAME_STATE,
            capabilities.python_frame_state,
            "operation requires observable Python frame stack, locals, and traceback line state",
        ),
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
        Role::IntegerLiteral => {
            if !op_has_integer_literal_payload(op)
                || capabilities.arbitrary_precision_integers
                || capabilities
                    .exact_integer_literal_max_magnitude
                    .is_some_and(|max| exact_integer_literal_value(op, max).is_some())
            {
                None
            } else {
                Some(
                    "integer literal exceeds the target's exact concrete value authority and requires the canonical arbitrary-precision value authority",
                )
            }
        }
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

fn op_has_integer_literal_payload(op: &OpIR) -> bool {
    match op.kind.as_str() {
        "const" => op.value.is_some(),
        "const_int" | "const_bigint" => true,
        _ => false,
    }
}

/// Return a canonical concrete integer literal when its complete decimal value
/// fits the target's exact carrier. This is shared by target admission and
/// bounded-value emitters so a backend cannot admit one range and materialize
/// another.
pub fn exact_integer_literal_value(op: &OpIR, max_magnitude: u128) -> Option<i128> {
    let value = match op.kind.as_str() {
        "const" | "const_int" => i128::from(op.value?),
        "const_bigint" => op.s_value.as_deref()?.parse::<i128>().ok()?,
        _ => return None,
    };
    (value.unsigned_abs() <= max_magnitude).then_some(value)
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

    #[test]
    fn exact_literal_capability_admits_only_concrete_in_range_siblings() {
        let capabilities = NumericTargetCapabilities::LUAU_EXACT_INTEGER_LITERALS;
        for op in [
            OpIR {
                kind: "const".to_string(),
                value: Some(42),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".to_string(),
                value: Some(-(1_i64 << 53)),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_bigint".to_string(),
                s_value: Some((1_u64 << 53).to_string()),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
        ] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "f".to_string(),
                    params: vec![],
                    ops: vec![op],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                }],
                profile: None,
            };
            validate_numeric_target_contract(&ir, "luau", capabilities)
                .expect("exact concrete literal must be admitted");
        }

        for payload in ["9007199254740993", "-9007199254740993", "not-an-int"] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "f".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "const_bigint".to_string(),
                        s_value: Some(payload.to_string()),
                        out: Some("out".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                }],
                profile: None,
            };
            let error = validate_numeric_target_contract(&ir, "luau", capabilities)
                .expect_err("unsafe or malformed bigint literal must reject");
            assert!(error.contains("exact concrete value authority"));
        }
    }

    #[test]
    fn generic_const_non_integer_payload_is_not_an_integer_literal() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "f".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "const".to_string(),
                    f_value: Some(1.25),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };
        validate_numeric_target_contract(
            &ir,
            "fixed",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect("generic const float payload must stay outside integer admission");
    }

    #[test]
    fn frame_state_siblings_share_one_effectful_capability() {
        for kind in ["frame_locals_set", "line", "trace_enter_slot", "trace_exit"] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "frame_state".to_string(),
                    params: vec!["locals".to_string()],
                    ops: vec![OpIR {
                        kind: kind.to_string(),
                        args: (kind == "frame_locals_set").then(|| vec!["locals".to_string()]),
                        value: matches!(kind, "line" | "trace_enter_slot").then_some(7),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                }],
                profile: None,
            };

            let error =
                validate_runtime_target_contract(&ir, "no-frames", RuntimeTargetCapabilities::NONE)
                    .expect_err("observable frame state must not degrade to a target no-op");
            assert!(error.contains("frame stack, locals, and traceback line"));

            validate_runtime_target_contract(
                &ir,
                "frames",
                RuntimeTargetCapabilities {
                    python_frame_state: true,
                    ..RuntimeTargetCapabilities::NONE
                },
            )
            .expect("the shared frame-state capability admits every sibling");
        }
    }
}
