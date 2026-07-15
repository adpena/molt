mod cfg;
mod construction;
mod facts;
mod locals;
mod repr;
mod runtime_ops;

use crate::wasm::body::WasmBodyOps;
use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::lir::{LirFunction, LirRepr};
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;
use std::collections::{HashMap, HashSet};
use wasm_encoder::ValType;

pub(super) use repr::lir_repr_to_val;

pub(super) struct LirLowerCtx<'a> {
    pub(super) func: &'a LirFunction,
    pub(super) value_locals: HashMap<ValueId, u32>,
    pub(super) value_reprs: HashMap<ValueId, LirRepr>,
    pub(super) value_types: HashMap<ValueId, TirType>,
    flat_list_int_values: HashSet<ValueId>,
    /// Reverse map: local index -> ValType. Built during allocation so the
    /// locals vector can be constructed in O(N) instead of O(N^2).
    pub(super) local_types: HashMap<u32, ValType>,
    pub(super) next_local: u32,
    pub(super) instructions: WasmBodyOps,
    pub(super) rpo: Vec<BlockId>,
    pub(super) block_index: HashMap<BlockId, usize>,
}
