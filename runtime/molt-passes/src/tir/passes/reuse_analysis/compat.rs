use crate::tir::types::TirType;

/// Allocation size class for reuse compatibility.
///
/// Two types are reuse-compatible iff they map to the same size class. This is
/// conservative: we only match types that are structurally identical or belong
/// to known fixed-size categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum SizeClass {
    /// A specific known type with fixed allocation size.
    Typed(TirType),
    /// Dynamic/unknown size - never matches anything.
    Dynamic,
}

/// Map a TIR type to its allocation size class.
pub(super) fn size_class(ty: &TirType) -> SizeClass {
    match ty {
        TirType::I64 | TirType::F64 | TirType::Bool | TirType::None | TirType::Never => {
            SizeClass::Dynamic
        }
        TirType::Str
        | TirType::Bytes
        | TirType::BigInt
        | TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Set(_) => SizeClass::Typed(ty.clone()),
        TirType::Tuple(elems) => SizeClass::Typed(TirType::Tuple(
            elems.iter().map(|_| TirType::DynBox).collect(),
        )),
        TirType::Box(_) | TirType::DynBox => SizeClass::Typed(TirType::DynBox),
        TirType::UserClass(_) => SizeClass::Typed(ty.clone()),
        TirType::Iterator(_) => SizeClass::Dynamic,
        TirType::Func(_) => SizeClass::Typed(ty.clone()),
        TirType::Ptr(_) => SizeClass::Dynamic,
        TirType::Union(_) => SizeClass::Dynamic,
    }
}

fn lists_compatible(a: &TirType, b: &TirType) -> bool {
    matches!((a, b), (TirType::List(_), TirType::List(_)))
}

fn dicts_compatible(a: &TirType, b: &TirType) -> bool {
    matches!((a, b), (TirType::Dict(_, _), TirType::Dict(_, _)))
}

/// Returns `true` if two types are reuse-compatible.
pub(super) fn reuse_compatible(a: &TirType, b: &TirType) -> bool {
    if a == b {
        return true;
    }
    if lists_compatible(a, b) || dicts_compatible(a, b) {
        return true;
    }
    let sa = size_class(a);
    let sb = size_class(b);
    if sa == SizeClass::Dynamic || sb == SizeClass::Dynamic {
        return false;
    }
    sa == sb
}
