use super::NumericTargetCapabilities;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::op_kinds_generated::{SimpleIrIntegerSemantics, simpleir_integer_semantics_table};
use crate::{OpIR, SimpleIR};

/// Reject unsupported numeric semantics before a backend assembles source.
/// Operation membership comes exclusively from the generated op-kind registry;
/// target policy supplies only capabilities.
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
/// fits the target's exact carrier.
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
