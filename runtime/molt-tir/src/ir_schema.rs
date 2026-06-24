use crate::OpIR;
use crate::tir::passes::effects::{EffectProof, simple_ir_effect_proof};

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
    "ge",
    "gt",
    "identity_alias",
    "binding_alias",
    "inplace_add",
    "inplace_div",
    "inplace_mul",
    "inplace_sub",
    "le",
    "load_var",
    "lt",
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
const ARENA_ELIGIBLE_KINDS: &[&str] = &[
    "alloc",
    "alloc_class",
    "alloc_class_static",
    "alloc_class_trusted",
    "object_new_bound",
];
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
