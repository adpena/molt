use super::WasmFrameLocals;
use wasm_encoder::ValType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameAnonymousLocal {
    DispatchSelfPtr,
    DispatchState,
    DispatchResumeState,
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
    ConstLiteralAnchor,
}

impl WasmFrameAnonymousLocal {
    fn val_type(self) -> ValType {
        ValType::I64
    }
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn allocate_anonymous(
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
    use super::{WasmFrameAnonymousLocal, WasmFrameLocals};
    use wasm_encoder::ValType;

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
        assert_eq!(locals[WasmFrameLocals::NONE_NAME], 3);
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
            locals.anonymous_kind(dispatch.resume_state_local.unwrap()),
            Some(WasmFrameAnonymousLocal::DispatchResumeState)
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
                ValType::I64,
            ]
        );
        assert_eq!(local_count, 13);
    }

    #[test]
    fn singleton_operand_and_discard_result_have_distinct_slots() {
        use crate::wasm::WasmFrameSyntheticLocal;

        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;
        let sink = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::DeadSink,
            &mut local_types,
            &mut local_count,
        );
        let cache = locals.allocate_constant_cache(0, &mut local_types, &mut local_count);
        let singleton = locals[WasmFrameLocals::NONE_NAME];
        assert_eq!(Some(singleton), cache.none_bits);
        assert_ne!(singleton, sink);
        assert_eq!(locals.result_slot(WasmFrameLocals::NONE_NAME), sink);
        locals.insert("value".into(), local_count);
        assert_eq!(locals.result_slot("value"), locals["value"]);
        locals.insert_dead_sink_alias("unused".into(), sink);
        assert_eq!(locals.bound_result_slot(Some("unused")), None);
        assert_eq!(locals.bound_result_slot(Some("none")), None);
        assert_eq!(locals.bound_result_slot(None), None);
        assert_eq!(locals.result_or_sink_slot(None), sink);
        assert_eq!(locals.result_or_sink_slot(Some("unused")), sink);
    }
}
