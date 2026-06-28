use super::{WasmFrameAnonymousLocal, WasmFrameLocals};
use wasm_encoder::ValType;

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmDispatchFrameLocals {
    pub(in crate::wasm) state_local: u32,
    pub(in crate::wasm) block_map_base_local: u32,
    pub(in crate::wasm) return_local: u32,
    pub(in crate::wasm) self_ptr_local: Option<u32>,
    pub(in crate::wasm) state_remap_base_local: Option<u32>,
    pub(in crate::wasm) state_remap_value_local: Option<u32>,
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn allocate_dispatch_locals(
        &mut self,
        stateful: bool,
        jumpful: bool,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> Option<WasmDispatchFrameLocals> {
        if !(stateful || jumpful) {
            return None;
        }
        let self_ptr_local = stateful.then(|| {
            self.allocate_anonymous(
                WasmFrameAnonymousLocal::DispatchSelfPtr,
                local_types,
                local_count,
            )
        });
        let state_local = self.allocate_anonymous(
            WasmFrameAnonymousLocal::DispatchState,
            local_types,
            local_count,
        );
        let block_map_base_local = self.allocate_anonymous(
            WasmFrameAnonymousLocal::DispatchBlockMapBase,
            local_types,
            local_count,
        );
        let return_local = self.allocate_anonymous(
            WasmFrameAnonymousLocal::DispatchReturn,
            local_types,
            local_count,
        );
        let state_remap_base_local = stateful.then(|| {
            self.allocate_anonymous(
                WasmFrameAnonymousLocal::DispatchStateRemapBase,
                local_types,
                local_count,
            )
        });
        let state_remap_value_local = stateful.then(|| {
            self.allocate_anonymous(
                WasmFrameAnonymousLocal::DispatchStateRemapValue,
                local_types,
                local_count,
            )
        });

        Some(WasmDispatchFrameLocals {
            state_local,
            block_map_base_local,
            return_local,
            self_ptr_local,
            state_remap_base_local,
            state_remap_value_local,
        })
    }
}
