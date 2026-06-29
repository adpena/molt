mod anonymous_locals;
mod constant_cache_locals;
mod dispatch_locals;
mod literal_scratch;
mod multi_return_locals;
mod synthetic_locals;

pub(in crate::wasm) use anonymous_locals::WasmFrameAnonymousLocal;
pub(in crate::wasm) use dispatch_locals::WasmDispatchFrameLocals;
#[cfg(test)]
pub(in crate::wasm) use literal_scratch::WasmLiteralPayload;
pub(in crate::wasm) use literal_scratch::{WasmLiteralScratchLocals, WasmLiteralScratchPolicy};
pub(in crate::wasm) use synthetic_locals::WasmFrameSyntheticLocal;

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

#[cfg(test)]
mod tests {
    use super::{WasmFrameLocalKind, WasmFrameLocals};
    use std::collections::BTreeMap;

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
}
