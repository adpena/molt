use super::*;

#[derive(Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct TrampolineKey {
    pub(crate) name: String,
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) is_import: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
}

pub(crate) mod function_compiler;
