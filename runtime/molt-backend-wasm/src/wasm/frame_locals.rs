use super::constant_ops::WasmConstOpPolicy;
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use crate::wasm_values::ConstantCache;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::ops::Index;
use wasm_encoder::ValType;

#[derive(Clone, Default)]
pub(in crate::wasm) struct WasmFrameLocals {
    slots: BTreeMap<String, u32>,
    name_kinds: BTreeMap<String, WasmFrameLocalKind>,
    anonymous_kinds: BTreeMap<u32, WasmFrameAnonymousLocal>,
    literal_scratch_policies: BTreeMap<String, WasmLiteralScratchPolicy>,
}

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmLiteralScratchLocals {
    ptr_local: u32,
    len_local: u32,
    policy: WasmLiteralScratchPolicy,
}

impl WasmLiteralScratchLocals {
    pub(in crate::wasm) fn ptr_local(self) -> u32 {
        self.ptr_local
    }

    pub(in crate::wasm) fn len_local(self) -> u32 {
        self.len_local
    }

    #[cfg(test)]
    pub(in crate::wasm) fn payload(self) -> WasmLiteralPayload {
        self.policy.payload()
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.policy.parse_scalar_eligible()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmLiteralPayload {
    None,
    String,
    Bytes,
    BigintDecimal,
}

impl WasmLiteralPayload {
    fn needs_literal_scratch(self) -> bool {
        !matches!(self, Self::None)
    }

    fn from_const_payload(payload: WasmConstLiteralPayload) -> Self {
        match payload {
            WasmConstLiteralPayload::None => Self::None,
            WasmConstLiteralPayload::String => Self::String,
            WasmConstLiteralPayload::Bytes => Self::Bytes,
            WasmConstLiteralPayload::BigintDecimal => Self::BigintDecimal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) struct WasmLiteralScratchPolicy {
    payload: WasmLiteralPayload,
    parse_scalar_eligible: bool,
}

impl WasmLiteralScratchPolicy {
    pub(in crate::wasm) fn new(payload: WasmLiteralPayload, parse_scalar_eligible: bool) -> Self {
        assert!(
            payload.needs_literal_scratch(),
            "literal scratch policy requires a typed literal payload"
        );
        assert!(
            !matches!(payload, WasmLiteralPayload::BigintDecimal) || !parse_scalar_eligible,
            "const_bigint decimal literal scratch must not be scalar-parse eligible"
        );
        Self {
            payload,
            parse_scalar_eligible,
        }
    }

    pub(in crate::wasm) fn payload(self) -> WasmLiteralPayload {
        self.payload
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.parse_scalar_eligible
    }

    fn from_const_policy(policy: WasmConstOpPolicy) -> Option<Self> {
        if !policy.needs_literal_scratch() {
            return None;
        }
        let payload = WasmLiteralPayload::from_const_payload(policy.literal_payload());
        Some(Self::new(payload, policy.parse_scalar_literal()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameLocalKind {
    Value,
    NoneSingleton,
    FixedSynthetic(WasmFrameSyntheticLocal),
    LiteralScratchPtr,
    LiteralScratchLen,
    MultiReturnCalleeValue,
    MultiReturnCallValue,
}

impl WasmFrameLocalKind {
    pub(in crate::wasm) fn is_call_retention_exempt(self) -> bool {
        !matches!(self, Self::Value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) struct WasmNamedFrameLocal<'a> {
    name: &'a str,
    slot: u32,
    kind: WasmFrameLocalKind,
}

impl<'a> WasmNamedFrameLocal<'a> {
    pub(in crate::wasm) fn name(self) -> &'a str {
        self.name
    }

    pub(in crate::wasm) fn slot(self) -> u32 {
        self.slot
    }

    pub(in crate::wasm) fn kind(self) -> WasmFrameLocalKind {
        self.kind
    }
}

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
    pub(in crate::wasm) const NONE_NAME: &'static str = "none";
    pub(in crate::wasm) const SELF_PARAM_NAME: &'static str = "self_param";

    pub(in crate::wasm) fn new() -> Self {
        Self::default()
    }

    pub(in crate::wasm) fn insert(&mut self, name: String, slot: u32) -> Option<u32> {
        let kind = Self::value_kind_for_name(&name);
        self.insert_with_kind(name, slot, kind)
    }

    pub(in crate::wasm) fn get<Q>(&self, name: &Q) -> Option<&u32>
    where
        String: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.slots.get(name)
    }

    pub(in crate::wasm) fn contains_key<Q>(&self, name: &Q) -> bool
    where
        String: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.slots.contains_key(name)
    }

    pub(in crate::wasm) fn ensure_literal_scratch(
        &mut self,
        out_name: &str,
        payload: WasmLiteralPayload,
        parse_scalar_eligible: bool,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> WasmLiteralScratchLocals {
        let policy = WasmLiteralScratchPolicy::new(payload, parse_scalar_eligible);
        self.record_literal_scratch_policy(out_name, policy);
        let ptr_local = self.ensure_named_i64(
            Self::literal_ptr_name(out_name),
            WasmFrameLocalKind::LiteralScratchPtr,
            local_types,
            local_count,
        );
        let len_local = self.ensure_named_i64(
            Self::literal_len_name(out_name),
            WasmFrameLocalKind::LiteralScratchLen,
            local_types,
            local_count,
        );
        WasmLiteralScratchLocals {
            ptr_local,
            len_local,
            policy,
        }
    }

    pub(in crate::wasm) fn ensure_literal_scratch_for_policy(
        &mut self,
        out_name: &str,
        policy: WasmConstOpPolicy,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> Option<WasmLiteralScratchLocals> {
        WasmLiteralScratchPolicy::from_const_policy(policy).map(|literal_policy| {
            self.ensure_literal_scratch(
                out_name,
                literal_policy.payload(),
                literal_policy.parse_scalar_eligible(),
                local_types,
                local_count,
            )
        })
    }

    pub(in crate::wasm) fn literal_scratch(&self, out_name: &str) -> WasmLiteralScratchLocals {
        self.try_literal_scratch(out_name).unwrap_or_else(|| {
            panic!("wasm literal scratch locals for {out_name} are not allocated")
        })
    }

    pub(in crate::wasm) fn try_literal_scratch(
        &self,
        out_name: &str,
    ) -> Option<WasmLiteralScratchLocals> {
        let ptr_name = Self::literal_ptr_name(out_name);
        let len_name = Self::literal_len_name(out_name);
        let policy = self.literal_scratch_policies.get(out_name).copied()?;
        Some(WasmLiteralScratchLocals {
            ptr_local: self.get(ptr_name.as_str()).copied()?,
            len_local: self.get(len_name.as_str()).copied()?,
            policy,
        })
    }

    pub(in crate::wasm) fn try_parse_scalar_literal_scratch(
        &self,
        out_name: &str,
    ) -> Option<WasmLiteralScratchLocals> {
        self.try_literal_scratch(out_name)
            .filter(|scratch| scratch.parse_scalar_eligible())
    }

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

    pub(in crate::wasm) fn ensure_multi_return_callee_value(
        &mut self,
        index: usize,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_i64(
            Self::multi_return_callee_name(index),
            WasmFrameLocalKind::MultiReturnCalleeValue,
            local_types,
            local_count,
        )
    }

    pub(in crate::wasm) fn ensure_multi_return_call_value(
        &mut self,
        result_var: &str,
        index: usize,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_i64(
            Self::multi_return_call_name(result_var, index),
            WasmFrameLocalKind::MultiReturnCallValue,
            local_types,
            local_count,
        )
    }

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

    #[cfg(test)]
    pub(in crate::wasm) fn anonymous_kind(&self, slot: u32) -> Option<WasmFrameAnonymousLocal> {
        self.anonymous_kinds.get(&slot).copied()
    }

    pub(in crate::wasm) fn local_kind(&self, name: &str) -> Option<WasmFrameLocalKind> {
        self.name_kinds.get(name).copied()
    }

    pub(in crate::wasm) fn named_locals(&self) -> impl Iterator<Item = WasmNamedFrameLocal<'_>> {
        self.slots.iter().map(|(name, &slot)| {
            let kind = self
                .name_kinds
                .get(name)
                .copied()
                .unwrap_or(WasmFrameLocalKind::Value);
            WasmNamedFrameLocal {
                name: name.as_str(),
                slot,
                kind,
            }
        })
    }

    fn insert_with_kind(
        &mut self,
        name: String,
        slot: u32,
        kind: WasmFrameLocalKind,
    ) -> Option<u32> {
        self.name_kinds.insert(name.clone(), kind);
        self.slots.insert(name, slot)
    }

    fn value_kind_for_name(name: &str) -> WasmFrameLocalKind {
        if name == Self::NONE_NAME {
            WasmFrameLocalKind::NoneSingleton
        } else {
            WasmFrameLocalKind::Value
        }
    }

    fn record_literal_scratch_policy(&mut self, out_name: &str, policy: WasmLiteralScratchPolicy) {
        if let Some(existing) = self.literal_scratch_policies.get(out_name) {
            assert_eq!(
                *existing, policy,
                "wasm literal scratch policy for {out_name} changed"
            );
            return;
        }
        self.literal_scratch_policies
            .insert(out_name.to_string(), policy);
    }

    fn ensure_named_i64(
        &mut self,
        name: String,
        kind: WasmFrameLocalKind,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_local(name, kind, ValType::I64, local_types, local_count)
    }

    fn ensure_named_local(
        &mut self,
        name: String,
        kind: WasmFrameLocalKind,
        val_type: ValType,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        if let Some(&idx) = self.get(name.as_str()) {
            let existing_kind = self
                .local_kind(name.as_str())
                .unwrap_or(WasmFrameLocalKind::Value);
            assert_eq!(
                existing_kind, kind,
                "wasm frame local {name} cannot be reused as {kind:?}; already {existing_kind:?}"
            );
            return idx;
        }
        let idx = *local_count;
        self.insert_with_kind(name, idx, kind);
        local_types.push(val_type);
        *local_count += 1;
        idx
    }

    fn allocate_anonymous(
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

    fn literal_ptr_name(out_name: &str) -> String {
        format!("{out_name}_ptr")
    }

    fn literal_len_name(out_name: &str) -> String {
        format!("{out_name}_len")
    }

    fn multi_return_callee_name(index: usize) -> String {
        format!("__multi_ret_{index}")
    }

    fn multi_return_call_name(result_var: &str, index: usize) -> String {
        format!("__multi_call_{result_var}_{index}")
    }
}

impl From<BTreeMap<String, u32>> for WasmFrameLocals {
    fn from(slots: BTreeMap<String, u32>) -> Self {
        let name_kinds = slots
            .keys()
            .map(|name| (name.clone(), Self::value_kind_for_name(name)))
            .collect();
        Self {
            slots,
            name_kinds,
            anonymous_kinds: BTreeMap::new(),
            literal_scratch_policies: BTreeMap::new(),
        }
    }
}

impl Index<&str> for WasmFrameLocals {
    type Output = u32;

    fn index(&self, name: &str) -> &Self::Output {
        self.slots
            .get(name)
            .unwrap_or_else(|| panic!("wasm frame local {name} is not allocated"))
    }
}

impl Index<&String> for WasmFrameLocals {
    type Output = u32;

    fn index(&self, name: &String) -> &Self::Output {
        &self[name.as_str()]
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_scratch_locals_are_owned_and_reused_by_frame_locals() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let first = locals.ensure_literal_scratch(
            "payload",
            WasmLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );
        let second = locals.ensure_literal_scratch(
            "payload",
            WasmLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );
        let looked_up = locals.literal_scratch("payload");
        let maybe_lookup = locals.try_literal_scratch("payload");
        let parse_lookup = locals.try_parse_scalar_literal_scratch("payload");

        assert_eq!(first.ptr_local(), 0);
        assert_eq!(first.len_local(), 1);
        assert_eq!(first.payload(), WasmLiteralPayload::String);
        assert!(first.parse_scalar_eligible());
        assert_eq!(second.ptr_local(), first.ptr_local());
        assert_eq!(second.len_local(), first.len_local());
        assert_eq!(looked_up.ptr_local(), first.ptr_local());
        assert_eq!(looked_up.len_local(), first.len_local());
        assert_eq!(maybe_lookup.map(|scratch| scratch.ptr_local()), Some(0));
        assert_eq!(parse_lookup.map(|scratch| scratch.len_local()), Some(1));
        assert!(locals.try_literal_scratch("missing").is_none());
        assert!(locals.try_parse_scalar_literal_scratch("missing").is_none());
        assert_eq!(
            locals.local_kind("payload_ptr"),
            Some(WasmFrameLocalKind::LiteralScratchPtr)
        );
        assert_eq!(
            locals.local_kind("payload_len"),
            Some(WasmFrameLocalKind::LiteralScratchLen)
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "payload_ptr")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "payload_len")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
        assert_eq!(local_count, 2);
    }

    #[test]
    fn literal_scratch_policy_controls_scalar_parse_eligibility() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let string_scratch = locals
            .ensure_literal_scratch_for_policy(
                "text",
                WasmConstOpPolicy::for_kind("const_str").expect("const_str policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_str should allocate literal scratch");
        let bigint_scratch = locals
            .ensure_literal_scratch_for_policy(
                "digits",
                WasmConstOpPolicy::for_kind("const_bigint").expect("const_bigint policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_bigint should allocate literal scratch");
        let bytes_scratch = locals
            .ensure_literal_scratch_for_policy(
                "blob",
                WasmConstOpPolicy::for_kind("const_bytes").expect("const_bytes policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_bytes should allocate literal scratch");
        let none_scratch = locals.ensure_literal_scratch_for_policy(
            "none",
            WasmConstOpPolicy::for_kind("const_none").expect("const_none policy"),
            &mut local_types,
            &mut local_count,
        );

        assert_eq!(string_scratch.payload(), WasmLiteralPayload::String);
        assert!(string_scratch.parse_scalar_eligible());
        assert_eq!(bigint_scratch.payload(), WasmLiteralPayload::BigintDecimal);
        assert!(!bigint_scratch.parse_scalar_eligible());
        assert_eq!(bytes_scratch.payload(), WasmLiteralPayload::Bytes);
        assert!(bytes_scratch.parse_scalar_eligible());
        assert!(none_scratch.is_none());
        assert!(locals.try_parse_scalar_literal_scratch("text").is_some());
        assert!(locals.try_parse_scalar_literal_scratch("blob").is_some());
        assert!(locals.try_literal_scratch("digits").is_some());
        assert!(locals.try_parse_scalar_literal_scratch("digits").is_none());
        assert_eq!(
            locals
                .try_literal_scratch("digits")
                .map(|scratch| scratch.payload()),
            Some(WasmLiteralPayload::BigintDecimal)
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
            ]
        );
        assert_eq!(local_count, 6);
    }

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

    #[test]
    fn frame_locals_from_map_preserves_value_name_kinds() {
        let locals = WasmFrameLocals::from(BTreeMap::from([
            (WasmFrameLocals::NONE_NAME.to_string(), 0),
            ("__tmp0".to_string(), 1),
        ]));

        assert_eq!(
            locals.local_kind(WasmFrameLocals::NONE_NAME),
            Some(WasmFrameLocalKind::NoneSingleton)
        );
        assert_eq!(locals.local_kind("__tmp0"), Some(WasmFrameLocalKind::Value));
    }

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
