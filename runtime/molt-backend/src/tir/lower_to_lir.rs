//! First-step helpers for lowering typed TIR facts into representation-aware LIR.
//!
//! Task 1 only needs the core representation mapping and block-argument
//! translation. Operation-level lowering lands in Task 2.

use super::lir::{LirRepr, LirValue};
use super::values::TirValue;

pub fn lower_block_args(args: &[TirValue]) -> Vec<LirValue> {
    args.iter()
        .map(|arg| LirValue {
            id: arg.id,
            ty: arg.ty.clone(),
            repr: LirRepr::for_type(&arg.ty),
        })
        .collect()
}
