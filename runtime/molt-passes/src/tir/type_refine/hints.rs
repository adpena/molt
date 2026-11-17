use crate::tir::types::TirType;

pub(super) fn parse_return_type_str(name: &str) -> Option<TirType> {
    match TirType::from_type_hint(name) {
        TirType::DynBox => None,
        ty => Some(ty),
    }
}
