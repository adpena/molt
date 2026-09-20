use crate::OpIR;
use crate::native_callable_abi::{NATIVE_CALLABLE_ABI_CHOICES, parse_native_callable_abi};
use crate::tir::op_kinds_generated::{
    SimpleIrOpValueRule, SimpleIrReturnShape, SimpleIrRuntimeRequirements, SimpleIrVarFieldRole,
    simpleir_kind_may_carry_async_work_poll_marker,
    simpleir_kind_may_carry_runtime_requirement_bits, simpleir_kind_may_carry_runtime_symbol,
    simpleir_op_shape, simpleir_return_shape, simpleir_var_field_role_table,
};

const SCALAR_FAST_INT_KINDS: &[&str] = &[
    "abs",
    "add",
    "bit_and",
    "bit_or",
    "bit_xor",
    "bool",
    "builtin_abs",
    "builtin_bool",
    "const",
    "copy",
    "copy_var",
    "binding_alias",
    "div",
    "eq",
    "floordiv",
    "ge",
    "gpu_block_dim",
    "gpu_block_id",
    "gpu_grid_dim",
    "gpu_thread_id",
    "gt",
    "identity_alias",
    "index",
    "inplace_add",
    "inplace_bit_and",
    "inplace_bit_or",
    "inplace_bit_xor",
    "inplace_floordiv",
    "inplace_mod",
    "inplace_mul",
    "inplace_sub",
    "invert",
    "le",
    "len",
    "load_var",
    "loop_index_next",
    "loop_index_start",
    "lshift",
    "lt",
    "mod",
    "mul",
    "ne",
    "neg",
    "not",
    "pos",
    "rshift",
    "shl",
    "shr",
    "sub",
];

const SCALAR_FAST_FLOAT_KINDS: &[&str] = &[
    "abs",
    "add",
    "builtin_abs",
    "const_float",
    "copy",
    "copy_var",
    "div",
    "eq",
    "float_from_obj",
    "floordiv",
    "ge",
    "gt",
    "identity_alias",
    "binding_alias",
    "inplace_add",
    "inplace_div",
    "inplace_floordiv",
    "inplace_mod",
    "inplace_mul",
    "inplace_sub",
    "le",
    "load_var",
    "lt",
    "mod",
    "mul",
    "ne",
    "neg",
    "pos",
    "sub",
];

const CONTAINER_TYPES: &[&str] = &[
    "bytearray",
    "bytes",
    "dict",
    "frozenset",
    "list",
    "list_bool",
    "list_float",
    "range",
    "set",
    "str",
    "tuple",
];

const BCE_SAFE_KINDS: &[&str] = &["index", "store_index"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpShapeViolation {
    OperandCount {
        expected: usize,
        actual: Option<usize>,
    },
    MissingResult,
    NonNegativeValue {
        actual: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpShapeDiagnostic {
    pub family: &'static str,
    pub kind: &'static str,
    pub violation: OpShapeViolation,
}

impl std::fmt::Display for OpShapeDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[family={}] `{}` ", self.family, self.kind)?;
        match self.violation {
            OpShapeViolation::OperandCount { expected, actual } => {
                write!(f, "requires `args` length {expected}, found ")?;
                match actual {
                    Some(actual) => write!(f, "{actual}"),
                    None => write!(f, "none"),
                }
            }
            OpShapeViolation::MissingResult => write!(f, "requires non-`none` `out` destination"),
            OpShapeViolation::NonNegativeValue { actual } => {
                write!(
                    f,
                    "requires explicit nonnegative integer `value`, found {actual:?}"
                )
            }
        }
    }
}

impl std::error::Error for OpShapeDiagnostic {}

/// Validate one generated operation shape without assuming a complete program,
/// value definitions, slot-table initialization or execution-context ownership.
/// Both SimpleIR and preserved TIR operations project into this same checker.
pub fn validate_op_shape(
    kind: &str,
    operands: Option<usize>,
    has_result: bool,
    value: Option<i64>,
) -> Result<(), OpShapeDiagnostic> {
    let Some(shape) = simpleir_op_shape(kind) else {
        return Ok(());
    };
    let violation = if operands.unwrap_or(0) != shape.operands {
        Some(OpShapeViolation::OperandCount {
            expected: shape.operands,
            actual: operands,
        })
    } else if shape.requires_result && !has_result {
        Some(OpShapeViolation::MissingResult)
    } else if shape.value_rule == SimpleIrOpValueRule::NonNegative
        && !value.is_some_and(|value| value >= 0)
    {
        Some(OpShapeViolation::NonNegativeValue { actual: value })
    } else {
        None
    };
    match violation {
        Some(violation) => Err(OpShapeDiagnostic {
            family: shape.family,
            kind: shape.kind,
            violation,
        }),
        None => Ok(()),
    }
}

fn validate_simple_op_shape(op: &OpIR) -> Result<(), OpShapeDiagnostic> {
    validate_op_shape(
        &op.kind,
        op.args.as_ref().map(Vec::len),
        op.out
            .as_deref()
            .is_some_and(|out| !out.trim().is_empty() && out != "none"),
        op.value,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionOpShapeDiagnostic {
    pub function: String,
    pub op_index: usize,
    pub shape: OpShapeDiagnostic,
}

impl std::fmt::Display for FunctionOpShapeDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "function `{}` op#{}: {}",
            self.function, self.op_index, self.shape
        )
    }
}

impl std::error::Error for FunctionOpShapeDiagnostic {}

pub fn validate_function_op_shapes(
    func: &crate::FunctionIR,
) -> Result<(), FunctionOpShapeDiagnostic> {
    for (op_index, op) in func.ops.iter().enumerate() {
        validate_simple_op_shape(op).map_err(|shape| FunctionOpShapeDiagnostic {
            function: func.name.clone(),
            op_index,
            shape,
        })?;
    }
    Ok(())
}

pub fn validate_simple_ir_op_shapes(ir: &crate::SimpleIR) -> Result<(), FunctionOpShapeDiagnostic> {
    for func in &ir.functions {
        validate_function_op_shapes(func)?;
    }
    Ok(())
}

#[cfg(test)]
mod op_shape_tests {
    use super::*;
    use crate::tir::op_kinds_generated::SIMPLEIR_OP_SHAPES;

    #[test]
    fn generated_shapes_reject_incomplete_and_excess_payloads_without_defaults() {
        for shape in SIMPLEIR_OP_SHAPES {
            let value = (shape.value_rule == SimpleIrOpValueRule::NonNegative).then_some(0);
            assert!(validate_op_shape(shape.kind, Some(shape.operands), true, value).is_ok());
            assert!(matches!(
                validate_op_shape(shape.kind, Some(shape.operands + 1), true, value),
                Err(OpShapeDiagnostic {
                    violation: OpShapeViolation::OperandCount { .. },
                    ..
                })
            ));
            if shape.operands > 0 {
                for actual in [None, Some(shape.operands - 1)] {
                    assert!(matches!(
                        validate_op_shape(shape.kind, actual, true, value),
                        Err(OpShapeDiagnostic {
                            violation: OpShapeViolation::OperandCount { .. },
                            ..
                        })
                    ));
                }
            } else {
                assert!(validate_op_shape(shape.kind, None, true, value).is_ok());
            }
            assert_eq!(
                validate_op_shape(shape.kind, Some(shape.operands), false, value).is_ok(),
                !shape.requires_result
            );
            if shape.value_rule == SimpleIrOpValueRule::NonNegative {
                for value in [None, Some(-1), Some(i64::MIN)] {
                    assert!(matches!(
                        validate_op_shape(shape.kind, Some(shape.operands), true, value),
                        Err(OpShapeDiagnostic {
                            violation: OpShapeViolation::NonNegativeValue { .. },
                            ..
                        })
                    ));
                }
                assert!(
                    validate_op_shape(shape.kind, Some(shape.operands), true, Some(i64::MAX))
                        .is_ok()
                );
            }
        }
        // Source lines are not code-slot identities and retain their distinct policy.
        assert!(validate_op_shape("line", None, false, None).is_ok());
        assert!(validate_op_shape("code_new", Some(9), false, None).is_ok());
    }
}

pub(crate) fn validate_required_fields(op: &OpIR) -> Result<(), String> {
    match op.kind.as_str() {
        "store_init" => {
            return Err(
                "retired compiler operation `store_init`: emit `store`; fresh-slot initialization is derived from typed-slot ownership facts"
                    .into(),
            );
        }
        "guarded_field_init" => {
            return Err(
                "retired compiler operation `guarded_field_init`: emit `guarded_field_set`; initialization cannot be asserted by wire spelling"
                    .into(),
            );
        }
        _ => {}
    }
    if op.kind == "object_new_bound_stack" {
        return Err("retired compiler operation `object_new_bound_stack`: frame placement requires an owner-lifetime proof; use owned `object_new_bound` allocation".into());
    }
    validate_representation_fields(op)?;
    validate_simple_op_shape(op).map_err(|error| error.to_string())
}

pub(crate) fn validate_function_param_types(
    function_name: &str,
    params: &[String],
    param_types: Option<&[String]>,
) -> Result<(), String> {
    let Some(param_types) = param_types else {
        return Ok(());
    };
    if param_types.len() != params.len() {
        return Err(format!(
            "function `{function_name}` has {} params but {} param_types",
            params.len(),
            param_types.len()
        ));
    }
    for (idx, ty) in param_types.iter().enumerate() {
        validate_clean_symbol(
            ty,
            &format!("function `{function_name}` param_types[{idx}]"),
        )?;
    }
    Ok(())
}

fn validate_representation_fields(op: &OpIR) -> Result<(), String> {
    if simpleir_var_field_role_table(op.kind.as_str()) == SimpleIrVarFieldRole::Forbidden
        && op.var.is_some()
    {
        return Err(format!(
            "return-family op `{}` forbids `var`; `args` is the sole value carrier",
            op.kind
        ));
    }
    let args_len = op.args.as_ref().map_or(0, Vec::len);
    match simpleir_return_shape(op.kind.as_str()) {
        SimpleIrReturnShape::Value if args_len != 1 => {
            return Err(format!(
                "value return op `{}` requires exactly one `args` operand, found {args_len}",
                op.kind
            ));
        }
        SimpleIrReturnShape::Void if args_len != 0 => {
            return Err(format!(
                "void return op `{}` forbids `args` operands, found {args_len}",
                op.kind
            ));
        }
        _ => {}
    }
    if op.fast_int == Some(true) && op.fast_float == Some(true) {
        return Err(format!(
            "op `{}` cannot set both fast_int and fast_float",
            op.kind
        ));
    }
    if op.fast_int == Some(true) && !SCALAR_FAST_INT_KINDS.contains(&op.kind.as_str()) {
        return Err(format!(
            "op `{}` does not own fast_int scalar specialization",
            op.kind
        ));
    }
    if op.fast_float == Some(true) && !SCALAR_FAST_FLOAT_KINDS.contains(&op.kind.as_str()) {
        return Err(format!(
            "op `{}` does not own fast_float scalar specialization",
            op.kind
        ));
    }
    if let Some(container_type) = op.container_type.as_deref() {
        validate_clean_symbol(container_type, &format!("op `{}` container_type", op.kind))?;
        if !CONTAINER_TYPES.contains(&container_type) {
            return Err(format!(
                "op `{}` has unsupported container_type `{container_type}`",
                op.kind
            ));
        }
    }
    if matches!(op.kind.as_str(), "func_new" | "func_new_closure") {
        match (op.task_kind.as_deref(), op.task_closure_size) {
            (None, None) => {}
            (Some("generator" | "coroutine" | "async_generator"), Some(size)) if size >= 0 => {}
            (Some(kind), Some(_))
                if !matches!(kind, "generator" | "coroutine" | "async_generator") =>
            {
                return Err(format!(
                    "op `{}` has unsupported callable task_kind `{kind}`",
                    op.kind
                ));
            }
            (Some(_), Some(size)) => {
                return Err(format!(
                    "op `{}` has negative task_closure_size `{size}`",
                    op.kind
                ));
            }
            _ => {
                return Err(format!(
                    "op `{}` must carry task_kind and task_closure_size together",
                    op.kind
                ));
            }
        }
    } else if op.task_closure_size.is_some() {
        return Err(format!("op `{}` cannot carry task_closure_size", op.kind));
    }
    if op.bce_safe == Some(true) && !BCE_SAFE_KINDS.contains(&op.kind.as_str()) {
        return Err(format!("op `{}` cannot carry bce_safe", op.kind));
    }
    if op.arena_eligible.is_some() {
        return Err(format!(
            "op `{}` cannot carry arena_eligible: {}",
            op.kind,
            crate::tir::target_info::COMPILER_ARENA_PLACEMENT_UNSUPPORTED
        ));
    }
    if let Some(type_hint) = op.type_hint.as_deref() {
        validate_clean_symbol(type_hint, &format!("op `{}` type_hint", op.kind))?;
    }
    if op.kind == "builtin_func" {
        let name_arg_count = op.args.as_ref().map_or(0, Vec::len);
        match op.builtin_name.as_deref() {
            Some(builtin_name) => {
                validate_clean_symbol(builtin_name, "builtin_func builtin_name")?;
                match name_arg_count {
                    1 => {}
                    0 => {
                        return Err(
                            "builtin_func builtin_name requires exactly one name operand, found none"
                                .to_string(),
                        );
                    }
                    found => {
                        return Err(format!(
                            "builtin_func builtin_name requires exactly one name operand, found {found}",
                        ));
                    }
                }
            }
            None if name_arg_count == 0 => {}
            None => {
                return Err(format!(
                    "builtin_func name operand requires builtin_name metadata, found {name_arg_count} operand(s)",
                ));
            }
        }
    } else if op.builtin_name.is_some() {
        return Err(format!("op `{}` cannot carry builtin_name", op.kind));
    }
    if let Some(runtime_symbol) = op.runtime_symbol.as_deref() {
        validate_clean_symbol(runtime_symbol, &format!("op `{}` runtime_symbol", op.kind))?;
        if !simpleir_kind_may_carry_runtime_symbol(op.kind.as_str()) {
            return Err(format!("op `{}` cannot carry runtime_symbol", op.kind));
        }
    }
    if op.runtime_requirement_bits != 0 {
        if !simpleir_kind_may_carry_runtime_requirement_bits(op.kind.as_str()) {
            return Err(format!(
                "op `{}` cannot carry runtime_requirement_bits",
                op.kind
            ));
        }
        if SimpleIrRuntimeRequirements::from_bits(op.runtime_requirement_bits).is_none() {
            return Err(format!(
                "op `{}` carries unknown runtime_requirement_bits {}",
                op.kind, op.runtime_requirement_bits
            ));
        }
    }
    if op.async_work_poll && !simpleir_kind_may_carry_async_work_poll_marker(op.kind.as_str()) {
        return Err(format!("op `{}` cannot carry async_work_poll", op.kind));
    }
    validate_native_callable_fields(op)?;
    Ok(())
}

fn validate_native_callable_fields(op: &OpIR) -> Result<(), String> {
    let has_native_callable = op.native_callable_export.is_some()
        || op.native_callable_binding.is_some()
        || op.native_callable_symbol.is_some()
        || op.native_callable_abi.is_some();
    if !has_native_callable {
        return Ok(());
    }
    if op.kind != "invoke_ffi" {
        return Err(format!(
            "op `{}` cannot carry native callable export metadata",
            op.kind
        ));
    }
    let Some(export_name) = op.native_callable_export.as_deref() else {
        return Err(
            "invoke_ffi native callable export requires native_callable_export".to_string(),
        );
    };
    validate_clean_symbol(export_name, "invoke_ffi native_callable_export")?;
    let Some(binding) = op.native_callable_binding.as_deref() else {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` requires native_callable_binding"
        ));
    };
    if !matches!(binding, "module_attr" | "direct_symbol") {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` has unsupported binding `{binding}`"
        ));
    }
    let Some(abi) = op.native_callable_abi.as_deref() else {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` requires native_callable_abi"
        ));
    };
    validate_clean_symbol(abi, "invoke_ffi native_callable_abi")?;
    let Some(parsed_abi) = parse_native_callable_abi(abi) else {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` has unknown native_callable_abi `{abi}`; expected one of: {NATIVE_CALLABLE_ABI_CHOICES}"
        ));
    };
    if binding == "module_attr" && parsed_abi.requires_direct_symbol_binding() {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` uses module_attr direct-symbol ABI `{abi}`"
        ));
    }
    if binding == "direct_symbol" {
        let Some(symbol) = op.native_callable_symbol.as_deref() else {
            return Err(format!(
                "invoke_ffi native callable export `{export_name}` direct_symbol requires native_callable_symbol"
            ));
        };
        validate_clean_symbol(symbol, "invoke_ffi native_callable_symbol")?;
    }
    let Some(args) = op.args.as_ref() else {
        return Err(format!(
            "invoke_ffi native callable export `{export_name}` requires an args payload"
        ));
    };
    if let Some(fixed_payload_arity) = parsed_abi.fixed_arity() {
        let expected = fixed_payload_arity + usize::from(binding == "module_attr");
        if args.len() != expected {
            return Err(format!(
                "invoke_ffi native callable export `{export_name}` with ABI `{abi}` has {} argument(s), expected {expected}",
                args.len()
            ));
        }
    }
    Ok(())
}

fn validate_clean_symbol(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must be nonempty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(())
}
