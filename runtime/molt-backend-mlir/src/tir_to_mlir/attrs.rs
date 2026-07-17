use melior::{
    Context as MlirContext,
    ir::{
        Attribute, Identifier, Type,
        attribute::{FloatAttribute, IntegerAttribute, StringAttribute},
        r#type::IntegerType,
    },
};
use molt_backend::tir::ops::{AttrDict, AttrValue};

pub(super) fn mlir_attributes<'c>(
    ctx: &'c MlirContext,
    attrs: &AttrDict,
) -> Vec<(Identifier<'c>, Attribute<'c>)> {
    let i64_type: Type<'c> = IntegerType::new(ctx, 64).into();
    let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
    let f64_type = Type::float64(ctx);
    let mut ordered: Vec<_> = attrs
        .iter()
        .filter(|(name, _)| name.as_str() != "_original_kind")
        .collect();
    ordered.sort_by_key(|(name, _)| *name);
    ordered
        .into_iter()
        .map(|(name, value)| {
            let value: Attribute<'c> = match value {
                AttrValue::Int(value) => IntegerAttribute::new(i64_type, *value).into(),
                AttrValue::Float(value) => FloatAttribute::new(ctx, f64_type, *value).into(),
                AttrValue::Str(value) => StringAttribute::new(ctx, value).into(),
                AttrValue::Bool(value) => IntegerAttribute::new(i1_type, i64::from(*value)).into(),
                AttrValue::Bytes(value) => {
                    let encoded = value
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    StringAttribute::new(ctx, &encoded).into()
                }
            };
            (Identifier::new(ctx, name), value)
        })
        .collect()
}

pub(super) fn extract_int_attr(attrs: &AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key)? {
        AttrValue::Int(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn extract_float_attr(attrs: &AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key)? {
        AttrValue::Float(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn extract_bool_attr(attrs: &AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key)? {
        AttrValue::Bool(v) => Some(*v),
        _ => None,
    }
}
