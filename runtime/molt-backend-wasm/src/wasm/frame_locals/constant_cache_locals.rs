use super::{WasmFrameAnonymousLocal, WasmFrameLocals};
use crate::wasm_values::ConstantCache;
use wasm_encoder::ValType;

impl WasmFrameLocals {
    pub(in crate::wasm) fn allocate_constant_cache(
        &mut self,
        fast_int_count: usize,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> ConstantCache {
        let mut cache = ConstantCache::default();
        if fast_int_count >= 3 {
            cache.int_shift = Some(self.allocate_anonymous(
                WasmFrameAnonymousLocal::ConstIntShift,
                local_types,
                local_count,
            ));
            cache.int_min = Some(self.allocate_anonymous(
                WasmFrameAnonymousLocal::ConstIntMin,
                local_types,
                local_count,
            ));
            cache.int_max = Some(self.allocate_anonymous(
                WasmFrameAnonymousLocal::ConstIntMax,
                local_types,
                local_count,
            ));
        }
        cache.none_bits = Some(self.allocate_anonymous(
            WasmFrameAnonymousLocal::ConstNoneBits,
            local_types,
            local_count,
        ));
        cache.qnan_tag_mask = Some(self.allocate_anonymous(
            WasmFrameAnonymousLocal::ConstQnanTagMask,
            local_types,
            local_count,
        ));
        cache.qnan_tag_ptr = Some(self.allocate_anonymous(
            WasmFrameAnonymousLocal::ConstQnanTagPtr,
            local_types,
            local_count,
        ));
        cache
    }
}
