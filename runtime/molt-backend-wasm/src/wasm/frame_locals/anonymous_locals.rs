use super::WasmFrameLocals;
use wasm_encoder::ValType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameAnonymousLocal {
    DispatchSelfPtr,
    DispatchState,
    DispatchBlockMapBase,
    DispatchReturn,
    DispatchStateRemapBase,
    DispatchStateRemapValue,
    ConstIntShift,
    ConstIntMin,
    ConstIntMax,
    ConstNoneBits,
    ConstQnanTagMask,
    ConstQnanTagPtr,
}

impl WasmFrameAnonymousLocal {
    fn val_type(self) -> ValType {
        ValType::I64
    }
}

impl WasmFrameLocals {
    pub(super) fn allocate_anonymous(
        &mut self,
        kind: WasmFrameAnonymousLocal,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        let idx = *local_count;
        self.anonymous_kinds.insert(idx, kind);
        local_types.push(kind.val_type());
        *local_count += 1;
        idx
    }

    #[cfg(test)]
    pub(in crate::wasm) fn anonymous_kind(&self, slot: u32) -> Option<WasmFrameAnonymousLocal> {
        self.anonymous_kinds.get(&slot).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_frame_locals_are_allocated_with_purpose_metadata() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let const_cache = locals.allocate_constant_cache(3, &mut local_types, &mut local_count);
        let dispatch = locals
            .allocate_dispatch_locals(true, false, &mut local_types, &mut local_count)
            .expect("stateful dispatch locals should be allocated");

        assert_eq!(const_cache.int_shift, Some(0));
        assert_eq!(const_cache.int_min, Some(1));
        assert_eq!(const_cache.int_max, Some(2));
        assert_eq!(const_cache.none_bits, Some(3));
        assert_eq!(const_cache.qnan_tag_mask, Some(4));
        assert_eq!(const_cache.qnan_tag_ptr, Some(5));
        assert_eq!(
            locals.anonymous_kind(0),
            Some(WasmFrameAnonymousLocal::ConstIntShift)
        );
        assert_eq!(
            locals.anonymous_kind(5),
            Some(WasmFrameAnonymousLocal::ConstQnanTagPtr)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.self_ptr_local.unwrap()),
            Some(WasmFrameAnonymousLocal::DispatchSelfPtr)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.state_local),
            Some(WasmFrameAnonymousLocal::DispatchState)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.block_map_base_local),
            Some(WasmFrameAnonymousLocal::DispatchBlockMapBase)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.return_local),
            Some(WasmFrameAnonymousLocal::DispatchReturn)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.state_remap_base_local.unwrap()),
            Some(WasmFrameAnonymousLocal::DispatchStateRemapBase)
        );
        assert_eq!(
            locals.anonymous_kind(dispatch.state_remap_value_local.unwrap()),
            Some(WasmFrameAnonymousLocal::DispatchStateRemapValue)
        );
        assert_eq!(
            local_types,
            vec![
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
            ]
        );
        assert_eq!(local_count, 12);
    }
}
