//! Annotation decoding for SimpleIR to TIR lowering.
//!
//! These hints describe parameters, not exact producer provenance. Scalar
//! result and return contracts are projected from the assembled TIR authority.

use super::super::types::TirType;

/// Convert a string type annotation to a `TirType`.
pub(super) fn string_to_tir_type(s: &str) -> TirType {
    match s {
        "int" | "i64" => TirType::I64,
        "float" | "f64" => TirType::F64,
        _ => match TirType::from_type_hint(s) {
            TirType::UserClass(_) => TirType::DynBox,
            ty => ty,
        },
    }
}

pub(super) fn param_string_to_tir_type(s: &str) -> TirType {
    match s {
        "i64" => TirType::DynBox,
        _ => string_to_tir_type(s),
    }
}
