use crate::OpIR;

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
