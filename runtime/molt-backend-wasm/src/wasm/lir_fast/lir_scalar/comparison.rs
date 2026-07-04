mod identity;
mod numeric;

pub(in crate::wasm::lir_fast) use identity::emit_lir_identity_comparison;
pub(in crate::wasm::lir_fast) use numeric::emit_lir_comparison;
