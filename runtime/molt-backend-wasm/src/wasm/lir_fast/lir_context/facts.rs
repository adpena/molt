use super::LirLowerCtx;
use molt_tir::tir::lir::LirRepr;
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;

impl LirLowerCtx<'_> {
    pub(in crate::wasm::lir_fast) fn repr_of(&self, vid: ValueId) -> LirRepr {
        self.value_reprs
            .get(&vid)
            .copied()
            .unwrap_or(LirRepr::DynBox)
    }

    pub(in crate::wasm::lir_fast) fn type_of(&self, vid: ValueId) -> Option<&TirType> {
        self.value_types.get(&vid)
    }

    pub(in crate::wasm::lir_fast) fn has_flat_list_int_storage(&self, vid: ValueId) -> bool {
        self.func
            .container_storage
            .get(&vid)
            .is_some_and(|fact| fact.kind == molt_tir::repr::ContainerStorageKind::FlatListInt)
    }
}
