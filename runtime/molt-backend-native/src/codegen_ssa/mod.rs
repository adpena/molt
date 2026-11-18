//! The backend-generic codegen walker over final TIR (design:
//! `docs/design/foundation/codegen_walker.md`). Backends implement instruction
//! selection only (`CodegenBuilder`); what each TIR value is, how a carrier
//! converts, and how the walk visits a function, lives here once for every
//! native backend.

pub(crate) mod builder;
pub(crate) mod constants;
pub(crate) mod control;
pub(crate) mod custody;
#[cfg(test)]
pub(crate) mod differential;
pub(crate) mod function;
pub(crate) mod numeric;
pub(crate) mod object;
pub(crate) mod operators;
pub(crate) mod scalar;
pub(crate) mod value_table;

pub(crate) use builder::{
    CheckedBinary, CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison,
    MachineKind, SiteBody,
};
pub(crate) use value_table::{HeldCarrier, ValueTable};
