//! The backend-generic codegen walker over final TIR (design:
//! `docs/design/foundation/codegen_walker.md`). Backends implement instruction
//! selection only; what each TIR value is, and how the walk visits a function,
//! lives here once for every native backend.

pub(crate) mod value_table;

pub(crate) use value_table::{Repr, ValueTable};
