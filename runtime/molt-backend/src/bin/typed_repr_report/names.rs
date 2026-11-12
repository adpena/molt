use molt_backend::tir::types::TirType;
use molt_ir::repr::Repr;

/// Report spelling of a carrier. `dynbox` is the name call_fact_coverage.py
/// treats as untyped.
pub(crate) fn repr_name(repr: Repr) -> &'static str {
    match repr {
        Repr::Never => "never",
        Repr::RawI64FullDeopt => "raw_i64_full_deopt",
        Repr::RawI64Safe => "raw_i64_safe",
        Repr::Bool => "bool",
        Repr::MaybeBigInt => "maybe_bigint",
        Repr::FloatUnboxed => "float_unboxed",
        Repr::BoxedImmortal => "boxed_immortal",
        Repr::DynBox => "dynbox",
    }
}

pub(crate) fn type_name(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "i64".to_string(),
        TirType::F64 => "f64".to_string(),
        TirType::Bool => "bool".to_string(),
        TirType::None => "none".to_string(),
        TirType::Str => "str".to_string(),
        TirType::Bytes => "bytes".to_string(),
        TirType::List(inner) => format!("list[{}]", type_name(inner)),
        TirType::Dict(key, value) => format!("dict[{},{}]", type_name(key), type_name(value)),
        TirType::Set(inner) => format!("set[{}]", type_name(inner)),
        TirType::Tuple(items) => {
            let inner = items.iter().map(type_name).collect::<Vec<_>>().join(",");
            format!("tuple[{inner}]")
        }
        TirType::Iterator(inner) => format!("iterator[{}]", type_name(inner)),
        TirType::Box(inner) => format!("box[{}]", type_name(inner)),
        TirType::DynBox => "dynbox".to_string(),
        TirType::UserClass(name) => format!("userclass[{name}]"),
        TirType::Func(signature) => format!(
            "func[({})->{}]",
            signature
                .params
                .iter()
                .map(type_name)
                .collect::<Vec<_>>()
                .join(","),
            type_name(&signature.return_type)
        ),
        TirType::BigInt => "bigint".to_string(),
        TirType::Ptr(inner) => format!("ptr[{}]", type_name(inner)),
        TirType::Union(items) => {
            let inner = items.iter().map(type_name).collect::<Vec<_>>().join("|");
            format!("union[{inner}]")
        }
        TirType::Never => "never".to_string(),
    }
}
