use super::{WasmFrameLocalKind, WasmFrameLocals};
use wasm_encoder::ValType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameSyntheticLocal {
    DeadSink,
    MoltTmp0,
    MoltTmp1,
    MoltTmp2,
    MoltTmp3,
    WasmTmp0,
    WasmTmp1,
    WasmAllocResolve,
    WasmScopeArena,
}

impl WasmFrameSyntheticLocal {
    pub(in crate::wasm) const MOLT_SCRATCH: [Self; 4] = [
        Self::MoltTmp0,
        Self::MoltTmp1,
        Self::MoltTmp2,
        Self::MoltTmp3,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::DeadSink => "__dead_sink",
            Self::MoltTmp0 => "__molt_tmp0",
            Self::MoltTmp1 => "__molt_tmp1",
            Self::MoltTmp2 => "__molt_tmp2",
            Self::MoltTmp3 => "__molt_tmp3",
            Self::WasmTmp0 => "__wasm_tmp0",
            Self::WasmTmp1 => "__wasm_tmp1",
            Self::WasmAllocResolve => "__wasm_alloc_resolve",
            Self::WasmScopeArena => "__wasm_scope_arena",
        }
    }

    fn val_type(self) -> ValType {
        match self {
            Self::WasmTmp0 | Self::WasmAllocResolve => ValType::I32,
            Self::DeadSink
            | Self::MoltTmp0
            | Self::MoltTmp1
            | Self::MoltTmp2
            | Self::MoltTmp3
            | Self::WasmTmp1
            | Self::WasmScopeArena => ValType::I64,
        }
    }
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn ensure_synthetic(
        &mut self,
        synthetic: WasmFrameSyntheticLocal,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_local(
            synthetic.name().to_string(),
            WasmFrameLocalKind::FixedSynthetic(synthetic),
            synthetic.val_type(),
            local_types,
            local_count,
        )
    }

    pub(in crate::wasm) fn synthetic(&self, synthetic: WasmFrameSyntheticLocal) -> u32 {
        self[synthetic.name()]
    }
}

#[cfg(test)]
mod tests {
    use super::{WasmFrameLocalKind, WasmFrameLocals, WasmFrameSyntheticLocal};
    use wasm_encoder::ValType;

    #[test]
    fn synthetic_locals_are_typed_and_classified_by_frame_locals() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let dead_sink = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::DeadSink,
            &mut local_types,
            &mut local_count,
        );
        let wasm_tmp0 = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::WasmTmp0,
            &mut local_types,
            &mut local_count,
        );
        let wasm_tmp1 = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::WasmTmp1,
            &mut local_types,
            &mut local_count,
        );
        let alloc_resolve = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::WasmAllocResolve,
            &mut local_types,
            &mut local_count,
        );
        let scope_arena = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::WasmScopeArena,
            &mut local_types,
            &mut local_count,
        );
        let molt_tmp0 = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::MoltTmp0,
            &mut local_types,
            &mut local_count,
        );
        let molt_tmp0_again = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::MoltTmp0,
            &mut local_types,
            &mut local_count,
        );

        assert_eq!(dead_sink, 0);
        assert_eq!(wasm_tmp0, 1);
        assert_eq!(wasm_tmp1, 2);
        assert_eq!(alloc_resolve, 3);
        assert_eq!(scope_arena, 4);
        assert_eq!(molt_tmp0, 5);
        assert_eq!(molt_tmp0_again, molt_tmp0);
        assert_eq!(
            locals.synthetic(WasmFrameSyntheticLocal::WasmTmp0),
            wasm_tmp0
        );
        assert_eq!(
            local_types,
            vec![
                ValType::I64,
                ValType::I32,
                ValType::I64,
                ValType::I32,
                ValType::I64,
                ValType::I64,
            ]
        );
        assert_eq!(local_count, 6);

        assert_eq!(
            locals.local_kind("__molt_tmp0"),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::MoltTmp0
            ))
        );
        assert_eq!(
            locals.local_kind("__wasm_tmp0"),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::WasmTmp0
            ))
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "__molt_tmp0")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "__wasm_tmp0")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        locals.insert(WasmFrameLocals::NONE_NAME.to_string(), 6);
        assert_eq!(
            locals.local_kind(WasmFrameLocals::NONE_NAME),
            Some(WasmFrameLocalKind::NoneSingleton)
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == WasmFrameLocals::NONE_NAME)
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        locals.insert("__tmp0".to_string(), 7);
        assert_eq!(locals.local_kind("__tmp0"), Some(WasmFrameLocalKind::Value));
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "__tmp0")
                .is_some_and(|local| !local.kind().is_call_retention_exempt())
        );
    }
}
