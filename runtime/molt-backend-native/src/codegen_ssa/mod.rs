//! The backend-generic codegen walker over final TIR (design:
//! `docs/design/foundation/codegen_walker.md`). Backends implement instruction
//! selection only (`CodegenBuilder`); what each TIR value is, how a carrier
//! converts, and how the walk visits a function, lives here once for every
//! native backend.

pub(crate) mod builder;
pub(crate) mod callable;
pub(crate) mod calls;
pub(crate) mod constants;
pub(crate) mod containers;
pub(crate) mod control;
pub(crate) mod custody;
#[cfg(test)]
pub(crate) mod differential;
pub(crate) mod direct;
pub(crate) mod frame;
pub(crate) mod function;
pub(crate) mod globals;
pub(crate) mod numeric;
pub(crate) mod object;
pub(crate) mod operators;
pub(crate) mod pending;
pub(crate) mod returns;
#[cfg(test)]
pub(crate) mod runtime_stubs;
pub(crate) mod scalar;
pub(crate) mod state;
pub(crate) mod task;
pub(crate) mod value_table;
pub(crate) mod walk;

pub(crate) use builder::{
    CheckedBinary, CodegenBuilder, FloatBinary, FloatComparison, HelperBody, IntBinary,
    IntComparison, MachineKind, RecursionCharge, SiteBody,
};
pub(crate) use value_table::{HeldCarrier, ValueTable};
