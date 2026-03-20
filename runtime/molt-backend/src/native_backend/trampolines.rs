use super::super::TrampolineKind;

#[derive(Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct TrampolineKey {
    pub(super) name: String,
    pub(super) arity: usize,
    pub(super) has_closure: bool,
    pub(super) is_import: bool,
    pub(super) kind: TrampolineKind,
    pub(super) closure_size: i64,
}
