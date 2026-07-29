use crate::OpIR;
use crate::native_callable_abi::{NATIVE_CALLABLE_ABI_CHOICES, parse_native_callable_abi};
use crate::tir::effect_proof::{EffectProof, simple_ir_effect_proof};
use crate::tir::op_kinds_generated::{
    SimpleIrReturnShape, SimpleIrRuntimeRequirements, SimpleIrVarFieldRole,
    simpleir_kind_may_carry_runtime_requirement_bits, simpleir_kind_may_carry_runtime_symbol,
    simpleir_return_shape, simpleir_var_field_role_table,
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
const ARENA_ELIGIBLE_KINDS: &[&str] = &["alloc", "alloc_class", "object_new_bound"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpFieldSchema {
    pub family: &'static str,
    pub kind: &'static str,
    pub required_args_len: Option<usize>,
    pub requires_out_value: bool,
}

// Generated-style scaffold:
// keep op field requirements centralized to avoid stringly drift between
// lowering and backend codegen. This first slice only covers the range-fill
// op family and is intentionally additive.
const RANGE_FILL_OP_SCHEMAS: &[OpFieldSchema] = &[
    OpFieldSchema {
        family: "range_fill",
        kind: "list_repeat_range",
        required_args_len: Some(4),
        requires_out_value: true,
    },
    OpFieldSchema {
        family: "range_fill",
        kind: "bytearray_fill_range",
        required_args_len: Some(4),
        requires_out_value: false,
    },
];

const OP_FIELD_SCHEMAS: &[OpFieldSchema] = RANGE_FILL_OP_SCHEMAS;

fn schema_for_kind(kind: &str) -> Option<&'static OpFieldSchema> {
    OP_FIELD_SCHEMAS.iter().find(|schema| schema.kind == kind)
}

pub(crate) fn validate_required_fields(op: &OpIR) -> Result<(), String> {
    validate_representation_fields(op)?;
    let Some(schema) = schema_for_kind(op.kind.as_str()) else {
        return Ok(());
    };
    if let Some(required) = schema.required_args_len {
        match op.args.as_ref() {
            Some(args) if args.len() == required => {}
            Some(args) => {
                return Err(format!(
                    "[family={}] requires `args` length {}, found {}",
                    schema.family,
                    required,
                    args.len()
                ));
            }
            None => {
                return Err(format!(
                    "[family={}] requires `args` length {}, found none",
                    schema.family, required
                ));
            }
        }
    }
    if schema.requires_out_value {
        match op.out.as_deref() {
            Some(out) if !out.trim().is_empty() && out != "none" => {}
            _ => {
                return Err(format!(
                    "[family={}] requires non-`none` `out` destination",
                    schema.family
                ));
            }
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
    if op.bce_safe == Some(true) && !BCE_SAFE_KINDS.contains(&op.kind.as_str()) {
        return Err(format!("op `{}` cannot carry bce_safe", op.kind));
    }
    if op.arena_eligible == Some(true) && !ARENA_ELIGIBLE_KINDS.contains(&op.kind.as_str()) {
        return Err(format!("op `{}` cannot carry arena_eligible", op.kind));
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
    if let Some(effect_proof) = op.effect_proof.as_deref() {
        validate_clean_symbol(effect_proof, &format!("op `{}` effect_proof", op.kind))?;
        let Some(proof) = EffectProof::from_name(effect_proof) else {
            return Err(format!(
                "op `{}` cannot carry effect_proof `{effect_proof}`",
                op.kind
            ));
        };
        if effect_proof != proof.name()
            || simple_ir_effect_proof(&op.kind, Some(effect_proof)) != Some(proof)
        {
            return Err(format!(
                "op `{}` cannot carry effect_proof `{effect_proof}`",
                op.kind
            ));
        }
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
