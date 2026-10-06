use crate::native_callable_abi::{NATIVE_CALLABLE_ABI_CHOICES, parse_native_callable_abi};
use crate::tir::op_kinds_generated::{
    SimpleIrCallTargetRole, SimpleIrOpValueRule, SimpleIrReturnShape, SimpleIrRuntimeRequirements,
    SimpleIrVarFieldRole, kind_consumed_operand_table, kind_source_call_callable_operand,
    kind_source_call_first_adopted_operand, kind_to_opcode_table, simpleir_backend_private_owner,
    simpleir_backend_service_kind, simpleir_call_target_role,
    simpleir_kind_has_function_reference_s_value, simpleir_kind_is_registered,
    simpleir_kind_may_carry_async_work_poll_marker,
    simpleir_kind_may_carry_runtime_requirement_bits, simpleir_kind_may_carry_runtime_symbol,
    simpleir_op_shape, simpleir_return_shape, simpleir_var_field_role_table,
};
use crate::{OpIR, ParameterCustody};

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
    UnregisteredKind,
    BackendPrivate {
        backend: &'static str,
    },
    ForbiddenVar,
    OperandCount {
        expected: usize,
        actual: Option<usize>,
    },
    Retired {
        reason: &'static str,
    },
    NonNegativeValue {
        actual: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpShapeDiagnostic {
    pub family: &'static str,
    pub kind: String,
    pub violation: OpShapeViolation,
}

impl std::fmt::Display for OpShapeDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.violation {
            OpShapeViolation::Retired { reason } => {
                return write!(f, "retired compiler operation `{}`: {reason}", self.kind);
            }
            OpShapeViolation::UnregisteredKind => {
                return write!(f, "unregistered op kind `{}`", self.kind);
            }
            OpShapeViolation::BackendPrivate { backend } => {
                return write!(
                    f,
                    "backend-private op kind `{}` belongs to {backend} internal lowering and cannot enter an external SimpleIR program",
                    self.kind
                );
            }
            _ => {}
        }
        write!(f, "[family={}] `{}` ", self.family, self.kind)?;
        match self.violation {
            OpShapeViolation::ForbiddenVar => {
                write!(f, "forbids `var`; `args` is the sole input carrier")
            }
            OpShapeViolation::OperandCount { expected, actual } => {
                write!(f, "requires `args` length {expected}, found ")?;
                match actual {
                    Some(actual) => write!(f, "{actual}"),
                    None => write!(f, "none"),
                }
            }
            OpShapeViolation::Retired { .. }
            | OpShapeViolation::UnregisteredKind
            | OpShapeViolation::BackendPrivate { .. } => {
                unreachable!("admission diagnostic formatted above")
            }
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

/// Retirement applies to both wire spellings and preserved origins, regardless
/// of which typed opcode currently carries the operation.
pub(crate) fn validate_op_not_retired(kind: &str) -> Result<(), OpShapeDiagnostic> {
    let retired = match kind {
        "store_init" => Some((
            "store_init",
            "emit `store`; fresh-slot initialization is derived from typed-slot ownership facts",
        )),
        "guarded_field_init" => Some((
            "guarded_field_init",
            "emit `guarded_field_set`; initialization cannot be asserted by wire spelling",
        )),
        "object_new_bound_stack" => Some((
            "object_new_bound_stack",
            "frame placement requires an owner-lifetime proof; use owned `object_new_bound` allocation",
        )),
        "list_repeat_range" => Some((
            "list_repeat_range",
            "use canonical list construction, multiplication or comprehension lowering",
        )),
        _ => None,
    };
    if let Some((kind, reason)) = retired {
        return Err(OpShapeDiagnostic {
            family: "retired",
            kind: kind.into(),
            violation: OpShapeViolation::Retired { reason },
        });
    }
    Ok(())
}

/// Reject retired spellings and validate generated operand/integer shape without
/// assuming a complete program, value definitions, slot-table initialization or
/// execution-context ownership. Existing result-role/cardinality facts remain
/// the result authority; this checker does not duplicate them.
/// Both SimpleIR and preserved TIR operations project into this same checker.
pub fn validate_op_shape(
    kind: &str,
    operands: Option<usize>,
    value: Option<i64>,
) -> Result<(), OpShapeDiagnostic> {
    validate_op_not_retired(kind)?;
    let Some(shape) = simpleir_op_shape(kind) else {
        return Ok(());
    };
    let violation = if operands.unwrap_or(0) != shape.operands {
        Some(OpShapeViolation::OperandCount {
            expected: shape.operands,
            actual: operands,
        })
    } else if shape.value_rule == SimpleIrOpValueRule::NonNegative
        && value.is_none_or(|value| value < 0)
    {
        Some(OpShapeViolation::NonNegativeValue { actual: value })
    } else {
        None
    };
    match violation {
        Some(violation) => Err(OpShapeDiagnostic {
            family: shape.family,
            kind: shape.kind.into(),
            violation,
        }),
        None => Ok(()),
    }
}

pub(crate) fn validate_registered_op_kind(kind: &str) -> Result<(), OpShapeDiagnostic> {
    validate_op_not_retired(kind)?;
    if !simpleir_kind_is_registered(kind) {
        return Err(OpShapeDiagnostic {
            family: "registration",
            kind: kind.into(),
            violation: OpShapeViolation::UnregisteredKind,
        });
    }
    Ok(())
}

fn validate_simple_op_shape(op: &OpIR) -> Result<(), OpShapeDiagnostic> {
    validate_registered_op_kind(&op.kind)?;
    validate_op_shape(&op.kind, op.args.as_ref().map(Vec::len), op.value)?;
    if let Some(shape) = simpleir_op_shape(&op.kind)
        && simpleir_var_field_role_table(&op.kind) == SimpleIrVarFieldRole::Forbidden
        && op.var.is_some()
    {
        return Err(OpShapeDiagnostic {
            family: shape.family,
            kind: shape.kind.into(),
            violation: OpShapeViolation::ForbiddenVar,
        });
    }
    Ok(())
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

/// Admission for external programs, before any backend may prune or rewrite
/// their operations. Internal function lowering uses the shape validator above
/// and may re-lift explicitly registered backend-private carriers.
pub fn validate_simple_ir_op_shapes(ir: &crate::SimpleIR) -> Result<(), FunctionOpShapeDiagnostic> {
    for func in &ir.functions {
        validate_function_op_shapes(func)?;
        for (op_index, op) in func.ops.iter().enumerate() {
            if let Some(backend) = simpleir_backend_private_owner(&op.kind) {
                return Err(FunctionOpShapeDiagnostic {
                    function: func.name.clone(),
                    op_index,
                    shape: OpShapeDiagnostic {
                        family: "registration",
                        kind: op.kind.clone(),
                        violation: OpShapeViolation::BackendPrivate { backend },
                    },
                });
            }
        }
    }
    Ok(())
}

/// Validate the control-label transport of the typed StateDispatch terminator.
/// Source IR without a map is lifted before terminal activation lowering.
/// Explicit maps must cover the saved state of each executable suspension;
/// an omitted state is invalid, never permission to infer another dispatch map.
pub fn validate_state_dispatch(ops: &[OpIR]) -> Result<(), String> {
    use std::collections::BTreeSet;
    let mut labels = BTreeSet::new();
    let mut duplicate_labels = BTreeSet::new();
    for op in ops {
        if matches!(op.kind.as_str(), "label" | "state_label")
            && let Some(label) = op.value
            && !labels.insert(label)
        {
            duplicate_labels.insert(label);
        }
    }
    let mut switches = 0;
    for (index, op) in ops.iter().enumerate() {
        if op.kind == "state_switch" {
            switches += 1;
            if switches > 1 {
                return Err(format!("op#{index}: multiple state_switch dispatch sites"));
            }
        }
        let Some(targets) = &op.state_targets else {
            continue;
        };
        if op.kind != "state_switch" {
            return Err(format!(
                "op#{index}: state_targets requires state_switch, found `{}`",
                op.kind
            ));
        }
        let mut states = BTreeSet::new();
        for &(state, label) in targets {
            if !states.insert(state) {
                return Err(format!("op#{index}: duplicate saved state {state}"));
            }
            if !labels.contains(&label) || duplicate_labels.contains(&label) {
                return Err(format!(
                    "op#{index}: saved state {state} requires one control label {label}"
                ));
            }
        }
    }
    crate::simple_verify::validate_explicit_state_resume_coverage(ops)
}

pub(crate) fn validate_required_fields(op: &OpIR) -> Result<(), String> {
    if op.kind == "callargs_new" {
        op.call_argument_form()?;
    }
    // Interpret symbol metadata only on executable/callable carriers, never
    // arbitrary string literals. The service-to-kind relation has one owner.
    let direct_symbol = (simpleir_call_target_role(&op.kind).is_some()
        || simpleir_kind_has_function_reference_s_value(&op.kind)
        || op.kind == "builtin_func"
        || kind_to_opcode_table(&op.kind) == Some(crate::tir::ops::OpCode::CallBuiltin))
    .then_some(op.s_value.as_deref())
    .flatten();
    for symbol in [
        direct_symbol,
        op.runtime_symbol.as_deref(),
        op.builtin_name.as_deref(),
        op.native_callable_symbol.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(kind) = simpleir_backend_service_kind(symbol) {
            return Err(format!(
                "backend service `{symbol}` cannot use generic `{}` transport; use `{kind}` operation",
                op.kind
            ));
        }
    }
    validate_simple_op_shape(op).map_err(|error| error.to_string())?;
    crate::literal_payload::validate_simple_literal(op)?;
    validate_value_transport(op)?;
    validate_representation_fields(op)
}

/// Canonical field roles and alias facts govern value transport before any
/// backend sees it. A malformed transport is never a backend support decision.
fn validate_value_transport(op: &OpIR) -> Result<(), String> {
    use crate::tir::op_kinds_generated::{
        copy_kind_is_explicit_no_heap_move_table, copy_kind_mints_owned_alias_ref_table,
        opcode_fixed_result_count_table, simpleir_kind_is_structural,
    };
    use crate::tir::simple_def_use::{
        simple_ir_binding, visit_simple_ir_reads, visit_simple_ir_result_names,
    };

    let binding = simpleir_var_field_role_table(&op.kind) == SimpleIrVarFieldRole::Definition;
    if binding {
        let destination = simple_ir_binding(op).map(|binding| binding.destination);
        if destination.is_none_or(|name| name.is_empty() || name == "none") {
            return Err(format!(
                "{} requires a non-empty, non-reserved binding destination",
                op.kind
            ));
        }
    }
    if copy_kind_is_explicit_no_heap_move_table(&op.kind)
        || copy_kind_mints_owned_alias_ref_table(&op.kind)
    {
        // Ownership transparency identifies the result's alias, not the number
        // of reads. Runtime guards also read their expected tag. Their exact
        // generated shape owns that arity; ordinary copy transports are unary.
        let expected = simpleir_op_shape(&op.kind).map_or(1, |shape| shape.operands);
        let mut actual = 0;
        visit_simple_ir_reads(op, |_| actual += 1);
        if actual != expected {
            return Err(format!(
                "{} requires exactly {expected} semantic source operand(s), found {actual}",
                op.kind
            ));
        }
    }
    if simpleir_kind_is_structural(&op.kind)
        || kind_to_opcode_table(&op.kind).and_then(opcode_fixed_result_count_table) == Some(0)
    {
        let mut result = false;
        visit_simple_ir_result_names(op, |_| result = true);
        if result {
            return Err(format!("{} cannot declare a value result", op.kind));
        }
    }
    Ok(())
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
    validate_argument_custody(op)?;
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

/// Custody vectors have one encoding: absent (every position borrowed), or one
/// entry per position naming at least one transfer.
pub(crate) fn validate_custody_projection(
    custody: &[ParameterCustody],
    positions: usize,
    what: &str,
) -> Result<(), String> {
    if custody.len() != positions {
        return Err(format!(
            "{what} names {} entries for {positions} positions",
            custody.len()
        ));
    }
    if !custody.contains(&ParameterCustody::Transferred) {
        return Err(format!(
            "{what} transfers nothing; all-borrowed custody is absent"
        ));
    }
    Ok(())
}

/// Typed `argument_custody` marks a source Python call instruction, on a
/// spelling with a generated `[[source_call_kind]]` row. A raw direct call
/// carries its target's parameter custody, which the whole-document check
/// compares. A dynamic source call adopts every argument, as CPython's CALL
/// does, but never an operand before the row's first adopted one (a `super()`
/// class). Its callable goes with its call form: a builder call's form is its
/// builder's, checked against the builder's `callargs_new`, and every other
/// callable belongs to an ordinary call, which adopts it.
fn validate_argument_custody(op: &OpIR) -> Result<(), String> {
    let Some(custody) = op.argument_custody.as_deref() else {
        return Ok(());
    };
    let Some(first_adopted) = kind_source_call_first_adopted_operand(&op.kind) else {
        return Err(format!("op `{}` cannot carry argument_custody", op.kind));
    };
    validate_custody_projection(
        custody,
        op.args.as_ref().map_or(0, Vec::len),
        &format!("op `{}` argument_custody", op.kind),
    )?;
    if matches!(
        simpleir_call_target_role(&op.kind),
        Some(SimpleIrCallTargetRole::InternalRequired | SimpleIrCallTargetRole::ExternalOrRuntime)
    ) {
        return Ok(());
    }
    let callable = kind_source_call_callable_operand(&op.kind);
    let builder_call = kind_consumed_operand_table(&op.kind, custody.len()).is_some();
    for (position, &actual) in custody.iter().enumerate() {
        let expected = if position < first_adopted {
            ParameterCustody::Borrowed
        } else if builder_call && Some(position) == callable {
            continue;
        } else {
            ParameterCustody::Transferred
        };
        if actual != expected {
            return Err(format!(
                "op `{}` operand {position} custody {actual:?} disagrees with its source call, which {} it",
                op.kind,
                if expected == ParameterCustody::Transferred {
                    "adopts"
                } else {
                    "borrows"
                },
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

#[cfg(test)]
mod op_shape_tests {
    use super::*;
    use crate::tir::op_kinds_generated::SIMPLEIR_OP_SHAPES;

    #[test]
    fn generated_shapes_reject_incomplete_and_excess_payloads_without_defaults() {
        for shape in SIMPLEIR_OP_SHAPES {
            let value = (shape.value_rule == SimpleIrOpValueRule::NonNegative).then_some(0);
            assert!(validate_op_shape(shape.kind, Some(shape.operands), value).is_ok());
            assert!(matches!(
                validate_op_shape(shape.kind, Some(shape.operands + 1), value),
                Err(OpShapeDiagnostic {
                    violation: OpShapeViolation::OperandCount { .. },
                    ..
                })
            ));
            if shape.operands > 0 {
                for actual in [None, Some(shape.operands - 1)] {
                    assert!(matches!(
                        validate_op_shape(shape.kind, actual, value),
                        Err(OpShapeDiagnostic {
                            violation: OpShapeViolation::OperandCount { .. },
                            ..
                        })
                    ));
                }
            } else {
                assert!(validate_op_shape(shape.kind, None, value).is_ok());
            }
            if shape.value_rule == SimpleIrOpValueRule::NonNegative {
                for value in [None, Some(-1), Some(i64::MIN)] {
                    assert!(matches!(
                        validate_op_shape(shape.kind, Some(shape.operands), value),
                        Err(OpShapeDiagnostic {
                            violation: OpShapeViolation::NonNegativeValue { .. },
                            ..
                        })
                    ));
                }
                assert!(
                    validate_op_shape(shape.kind, Some(shape.operands), Some(i64::MAX)).is_ok()
                );
            }
        }
        // Source lines are not code-slot identities and retain their distinct policy.
        assert!(validate_op_shape("line", None, None).is_ok());
        assert!(validate_op_shape("code_new", Some(9), None).is_ok());
    }
}
