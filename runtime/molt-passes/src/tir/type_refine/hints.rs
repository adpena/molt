use crate::tir::ops::{AttrDict, AttrValue};
use crate::tir::types::TirType;

pub(super) fn parse_guard_type(attrs: &AttrDict) -> Option<TirType> {
    let type_str = attrs
        .get("expected_type")
        .or_else(|| attrs.get("ty"))
        .and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        })?;

    match type_str.to_ascii_lowercase().as_str() {
        "int" | "i64" => Some(TirType::I64),
        "float" | "f64" => Some(TirType::F64),
        "bool" => Some(TirType::Bool),
        "str" | "string" => Some(TirType::Str),
        "none" | "nonetype" => Some(TirType::None),
        "bytes" => Some(TirType::Bytes),
        "list" => Some(TirType::List(Box::new(TirType::DynBox))),
        "dict" => Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox),
        )),
        "set" => Some(TirType::Set(Box::new(TirType::DynBox))),
        "tuple" => Some(TirType::Tuple(vec![])),
        "bigint" => Some(TirType::BigInt),
        _ => None,
    }
}

pub(super) fn parse_return_type_str(name: &str) -> Option<TirType> {
    match TirType::from_type_hint(name) {
        TirType::DynBox => None,
        ty => Some(ty),
    }
}

pub(super) fn structural_builtin_return_type(name: &str) -> Option<TirType> {
    match name {
        "len" | "id" | "ord" => Some(TirType::I64),
        "bool" | "hasattr" | "isinstance" | "issubclass" => Some(TirType::Bool),
        "chr" => Some(TirType::Str),
        _ => None,
    }
}
