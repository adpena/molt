use super::constant_ops::{
    const_seed_bits, emit_seeded_runtime_const_op, needs_literal_pointer_locals,
    needs_seeded_runtime_const,
};
use super::context::CompileFuncContext;
use super::local_analysis::{LocalVariableAnalysis, analyze_local_variables};
use super::multi_return_layout::WasmMultiReturnLayout;
use super::state_dispatch::NonLinearDispatchLocals;
use super::*;
use std::borrow::Borrow;
use std::collections::btree_map::Iter;
use std::ops::Index;

#[derive(Clone, Default)]
pub(in crate::wasm) struct WasmFrameLocals {
    slots: BTreeMap<String, u32>,
    name_kinds: BTreeMap<String, WasmFrameLocalKind>,
    anonymous_kinds: BTreeMap<u32, WasmFrameAnonymousLocal>,
}

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmLiteralScratchLocals {
    ptr_local: u32,
    len_local: u32,
}

impl WasmLiteralScratchLocals {
    pub(in crate::wasm) fn ptr_local(self) -> u32 {
        self.ptr_local
    }

    pub(in crate::wasm) fn len_local(self) -> u32 {
        self.len_local
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameLocalKind {
    Value,
    FixedSynthetic(WasmFrameSyntheticLocal),
    LiteralScratchPtr,
    LiteralScratchLen,
    MultiReturnCalleeValue,
    MultiReturnCallValue,
}

impl WasmFrameLocalKind {
    fn is_call_retention_exempt(self) -> bool {
        !matches!(self, Self::Value)
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
    const MOLT_SCRATCH: [Self; 4] = [
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

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "__dead_sink" => Some(Self::DeadSink),
            "__molt_tmp0" => Some(Self::MoltTmp0),
            "__molt_tmp1" => Some(Self::MoltTmp1),
            "__molt_tmp2" => Some(Self::MoltTmp2),
            "__molt_tmp3" => Some(Self::MoltTmp3),
            "__wasm_tmp0" => Some(Self::WasmTmp0),
            "__wasm_tmp1" => Some(Self::WasmTmp1),
            "__wasm_alloc_resolve" => Some(Self::WasmAllocResolve),
            "__wasm_scope_arena" => Some(Self::WasmScopeArena),
            _ => None,
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
        self.insert_with_kind(name, slot, WasmFrameLocalKind::Value)
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
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> WasmLiteralScratchLocals {
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
        }
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
        Some(WasmLiteralScratchLocals {
            ptr_local: self.get(ptr_name.as_str()).copied()?,
            len_local: self.get(len_name.as_str()).copied()?,
        })
    }

    pub(in crate::wasm) fn is_literal_scratch_name(name: &str) -> bool {
        name.ends_with("_ptr") || name.ends_with("_len")
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

    pub(in crate::wasm) fn name_kind(&self, name: &str) -> Option<WasmFrameLocalKind> {
        self.name_kinds.get(name).copied()
    }

    pub(in crate::wasm) fn is_synthetic_name(name: &str) -> bool {
        WasmFrameSyntheticLocal::from_name(name).is_some()
    }

    pub(in crate::wasm) fn is_reserved_internal_name(name: &str) -> bool {
        Self::is_synthetic_name(name)
            || Self::is_literal_scratch_name(name)
            || name.starts_with("__multi_ret_")
            || name.starts_with("__multi_call_")
    }

    pub(in crate::wasm) fn is_call_retention_exempt_name(&self, name: &str) -> bool {
        name == Self::NONE_NAME
            || self
                .name_kind(name)
                .is_some_and(WasmFrameLocalKind::is_call_retention_exempt)
    }

    pub(in crate::wasm) fn is_coalescable_value_name(
        name: &str,
        read_vars: &BTreeSet<String>,
        param_set: &BTreeSet<String>,
    ) -> bool {
        (name.starts_with("__tmp") || name.starts_with("__v"))
            && !param_set.contains(name)
            && read_vars.contains(name)
            && !Self::is_reserved_internal_name(name)
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
            .map(|name| (name.clone(), WasmFrameLocalKind::Value))
            .collect();
        Self {
            slots,
            name_kinds,
            anonymous_kinds: BTreeMap::new(),
        }
    }
}

impl<'a> IntoIterator for &'a WasmFrameLocals {
    type Item = (&'a String, &'a u32);
    type IntoIter = Iter<'a, String, u32>;

    fn into_iter(self) -> Self::IntoIter {
        self.slots.iter()
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::wasm) enum WasmFrameControlMode {
    Plain,
    Jumpful,
    Stateful,
}

impl WasmFrameControlMode {
    pub(in crate::wasm) fn is_stateful(self) -> bool {
        matches!(self, Self::Stateful)
    }

    fn needs_dispatch(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

pub(super) struct WasmFunctionFramePlan {
    local_types: Vec<ValType>,
    frame: WasmFunctionFrame,
}

pub(super) struct WasmFunctionFrame {
    locals: WasmFrameLocals,
    runtime_lookup_only_vars: BTreeSet<String>,
    scalar_plan: ScalarRepresentationPlan,
    control_mode: WasmFrameControlMode,
    tail_call_eligible: bool,
    arena_local: Option<u32>,
    dispatch_locals: Option<WasmDispatchFrameLocals>,
    const_cache: ConstantCache,
    const_seed_locals: Vec<(u32, i64)>,
    seeded_runtime_const_ops: Vec<(usize, OpIR)>,
    seeded_runtime_const_op_indices: BTreeSet<usize>,
    multi_return: WasmMultiReturnLayout,
}

#[derive(Clone, Copy)]
struct WasmDispatchFrameLocals {
    state_local: u32,
    block_map_base_local: u32,
    return_local: u32,
    self_ptr_local: Option<u32>,
    state_remap_base_local: Option<u32>,
    state_remap_value_local: Option<u32>,
}

impl WasmFrameLocals {
    fn allocate_dispatch_locals(
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

#[derive(Clone, Copy)]
struct FrameLocalAllocationPolicy<'a> {
    read_vars: &'a BTreeSet<String>,
    param_set: &'a BTreeSet<String>,
    coalesced_map: &'a BTreeMap<String, String>,
    dead_sink_idx: u32,
}

fn ensure_frame_local(
    locals: &mut WasmFrameLocals,
    local_types: &mut Vec<ValType>,
    local_count: &mut u32,
    policy: FrameLocalAllocationPolicy<'_>,
    name: &str,
    as_dead_out: bool,
) -> u32 {
    if let Some(&idx) = locals.get(name) {
        return idx;
    }
    if as_dead_out && !policy.read_vars.contains(name) && !policy.param_set.contains(name) {
        locals.insert(name.to_string(), policy.dead_sink_idx);
        return policy.dead_sink_idx;
    }
    if let Some(repr) = policy.coalesced_map.get(name)
        && repr != name
        && let Some(&repr_idx) = locals.get(repr)
    {
        locals.insert(name.to_string(), repr_idx);
        return repr_idx;
    }
    let idx = *local_count;
    locals.insert(name.to_string(), idx);
    local_types.push(ValType::I64);
    *local_count += 1;
    idx
}

impl WasmFunctionFramePlan {
    pub(super) fn for_function(func_ir: &FunctionIR, ctx: &CompileFuncContext<'_>) -> Self {
        let mut locals = WasmFrameLocals::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if func_ir.name.ends_with("_poll") {
            let self_param_idx = locals.get("self").copied().unwrap_or(0);
            locals.insert(WasmFrameLocals::SELF_PARAM_NAME.to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
            if local_count == 0 {
                local_count = 1;
            }
        }

        let LocalVariableAnalysis {
            read_vars,
            param_set,
            runtime_lookup_only_vars,
            coalesced_map,
            defined_vars,
            used_vars,
        } = analyze_local_variables(func_ir);

        // Allocate a single shared dead-sink local for output-only variables.
        let dead_sink_idx = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::DeadSink,
            &mut local_types,
            &mut local_count,
        );

        let mut needs_field_fast = false;
        let mut needs_alloc_resolve = false;
        // Scope arena eligibility: any op marked `arena_eligible` triggers
        // a per-function ScopeArena lifecycle (arena_new at entry,
        // arena_alloc_object at every eligible alloc site, arena_free before
        // every return). Mirrors the native backend integration.
        let has_arena_eligible = func_ir.ops.iter().any(|op| op.arena_eligible == Some(true));
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        let mut stateful = false;
        let mut saw_jump_or_label = false;
        let mut fast_int_count: usize = 0;
        let mut const_seed_seen: BTreeSet<String> = BTreeSet::new();
        let mut const_seed_locals_all: Vec<(u32, i64)> = Vec::new();
        let mut seeded_runtime_const_ops: Vec<(usize, OpIR)> = Vec::new();
        let allocation_policy = FrameLocalAllocationPolicy {
            read_vars: &read_vars,
            param_set: &param_set,
            coalesced_map: &coalesced_map,
            dead_sink_idx,
        };
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                fast_int_count += 1;
            }
            if let Some(var) = &op.var {
                let var_is_dead_out = op.kind == "store_var";
                ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    var,
                    var_is_dead_out,
                );
            }
            if let Some(args) = &op.args {
                for arg in args {
                    ensure_frame_local(
                        &mut locals,
                        &mut local_types,
                        &mut local_count,
                        allocation_policy,
                        arg,
                        false,
                    );
                }
            }
            if let Some(out) = &op.out {
                let out_local_idx = ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    out,
                    true,
                );
                let is_dead = out_local_idx == dead_sink_idx;
                if needs_literal_pointer_locals(&op.kind) {
                    locals.ensure_literal_scratch(out, &mut local_types, &mut local_count);
                }
                if !const_seed_seen.contains(out) {
                    let bits = const_seed_bits(op);
                    if let Some(bits) = bits {
                        // Skip seeding dead locals -- the value is never
                        // observed so there is no point initializing it.
                        if !is_dead {
                            const_seed_seen.insert(out.clone());
                            const_seed_locals_all.push((out_local_idx, bits));
                        }
                    } else if needs_seeded_runtime_const(&op.kind) && !is_dead {
                        const_seed_seen.insert(out.clone());
                        seeded_runtime_const_ops.push((op_idx, op.clone()));
                    }
                }
            }
            match op.kind.as_str() {
                "store" | "store_init" | "load" | "guarded_load" | "guarded_field_get"
                | "guarded_field_set" | "guarded_field_init" => needs_field_fast = true,
                "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                | "chan_recv_yield" => stateful = true,
                "jump" | "label" => saw_jump_or_label = true,
                "alloc_task" => {
                    let tk = op.task_kind.as_deref().unwrap_or("future");
                    let has_prefix = tk == "generator";
                    let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                    if has_prefix || has_args {
                        needs_alloc_resolve = true;
                    }
                }
                _ => {}
            }
        }

        // Safety: seed undefined variables (used but never defined) with
        // box_none().  This can happen when front-end IR omits a const_none
        // definition due to module-context differences (e.g. genexpr compiled
        // for import vs __main__).  Without this, the WASM local defaults to
        // 0 which is not a valid boxed value and causes runtime crashes.
        for undef in used_vars.difference(&defined_vars) {
            if let Some(&local_idx) = locals.get(undef.as_str())
                && local_idx != dead_sink_idx
                && !param_set.contains(undef.as_str())
                && !const_seed_seen.contains(undef)
            {
                const_seed_seen.insert(undef.clone());
                const_seed_locals_all.push((local_idx, box_none()));
            }
        }

        if needs_field_fast {
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmTmp0,
                &mut local_types,
                &mut local_count,
            );
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmTmp1,
                &mut local_types,
                &mut local_count,
            );
        }

        if needs_alloc_resolve {
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmAllocResolve,
                &mut local_types,
                &mut local_count,
            );
        }

        // Reserve a slot to hold the ScopeArena handle returned by
        // `molt_arena_new`. The slot is initialised at function entry and
        // consumed by every arena-eligible alloc + every return site.
        let arena_local: Option<u32> = if has_arena_eligible {
            Some(locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmScopeArena,
                &mut local_types,
                &mut local_count,
            ))
        } else {
            None
        };

        for scratch in WasmFrameSyntheticLocal::MOLT_SCRATCH {
            locals.ensure_synthetic(scratch, &mut local_types, &mut local_count);
        }

        // Constant materialization cache: when a function body has 3+ fast_int
        // ops, pre-allocate WASM locals for the constants that would otherwise
        // be emitted as i64.const immediates dozens of times (INT_SHIFT,
        // INT_MIN_INLINE, INT_MAX_INLINE).  Below the threshold the overhead
        // of initializing the locals exceeds the savings.
        let const_cache =
            locals.allocate_constant_cache(fast_int_count, &mut local_types, &mut local_count);

        // Extended constant cache: cache box_none(), QNAN_TAG_MASK_I64, and
        // QNAN_TAG_PTR_I64 into locals unconditionally â€” these large i64
        // constants (9-10 bytes each as immediates) appear dozens of times in
        // every function body.  Replacing with local.get (1-2 bytes) saves
        // 7-8 bytes per occurrence.
        let jumpful = !stateful && saw_jump_or_label;

        // --- Tail call optimization eligibility (WASM tail calls proposal Â§3.5) ---
        // A function is eligible for tail call optimization when it is
        // non-stateful (stateful dispatch emits ops one-at-a-time).
        // Exception handling is checked per-call-site via try_stack
        // instead of blanket-disabling the whole function.
        let tail_call_eligible = !stateful;

        if stateful && !locals.contains_key(WasmFrameLocals::SELF_PARAM_NAME) {
            let self_param_idx = locals
                .get("self")
                .copied()
                .or_else(|| {
                    func_ir
                        .params
                        .first()
                        .and_then(|name| locals.get(name))
                        .copied()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing self parameter",
                        func_ir.name
                    )
                });
            locals.insert(WasmFrameLocals::SELF_PARAM_NAME.to_string(), self_param_idx);
            if !locals.contains_key("self") {
                locals.insert("self".to_string(), self_param_idx);
            }
        }
        let dispatch_locals =
            locals.allocate_dispatch_locals(stateful, jumpful, &mut local_types, &mut local_count);
        let const_seed_locals = if stateful || jumpful {
            const_seed_locals_all
        } else {
            Vec::new()
        };
        let seeded_runtime_const_ops = if stateful || jumpful {
            seeded_runtime_const_ops
        } else {
            Vec::new()
        };
        let seeded_runtime_const_op_indices: BTreeSet<usize> = seeded_runtime_const_ops
            .iter()
            .map(|(idx, _)| *idx)
            .collect();
        if std::env::var("MOLT_DEBUG_WASM_SEEDS_FUNC").ok().as_deref()
            == Some(func_ir.name.as_str())
        {
            eprintln!(
                "WASM_SEEDS_FUNC name={} seeds={:?} runtime_const_ops={}",
                func_ir.name,
                const_seed_locals,
                seeded_runtime_const_ops.len()
            );
            for name in &func_ir.params {
                if let Some(idx) = locals.get(name) {
                    eprintln!("WASM_SEEDS_PARAM name={} slot={}", name, idx);
                }
            }
            let mut slot_to_names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
            for (name, &idx) in &locals {
                slot_to_names.entry(idx).or_default().push(name.clone());
            }
            for (slot, _) in &const_seed_locals {
                if let Some(names) = slot_to_names.get(slot) {
                    eprintln!("WASM_SEEDS_SLOT slot={} names={:?}", slot, names);
                }
            }
        }

        let multi_return = WasmMultiReturnLayout::build(
            func_ir,
            ctx.multi_return_candidates,
            &mut locals,
            &mut local_types,
            &mut local_count,
        );

        let control_mode = if stateful {
            WasmFrameControlMode::Stateful
        } else if jumpful {
            WasmFrameControlMode::Jumpful
        } else {
            WasmFrameControlMode::Plain
        };
        debug_assert_eq!(control_mode.needs_dispatch(), dispatch_locals.is_some());

        let _ = local_count;
        Self {
            local_types,
            frame: WasmFunctionFrame {
                locals,
                runtime_lookup_only_vars,
                scalar_plan,
                control_mode,
                tail_call_eligible,
                arena_local,
                dispatch_locals,
                const_cache,
                const_seed_locals,
                seeded_runtime_const_ops,
                seeded_runtime_const_op_indices,
                multi_return,
            },
        }
    }

    pub(super) fn into_function_and_frame(self) -> (Function, WasmFunctionFrame) {
        (
            Function::new_with_locals_types(self.local_types),
            self.frame,
        )
    }
}

impl WasmFunctionFrame {
    pub(super) fn control_mode(&self) -> WasmFrameControlMode {
        self.control_mode
    }

    pub(super) fn dispatch_locals(&self) -> Option<NonLinearDispatchLocals> {
        self.dispatch_locals.map(|locals| NonLinearDispatchLocals {
            state_local: locals.state_local,
            block_map_base_local: locals.block_map_base_local,
            return_local: locals.return_local,
            self_ptr_local: locals.self_ptr_local,
            state_remap_base_local: locals.state_remap_base_local,
            state_remap_value_local: locals.state_remap_value_local,
        })
    }

    pub(super) fn locals(&self) -> &WasmFrameLocals {
        &self.locals
    }

    pub(super) fn runtime_lookup_only_vars(&self) -> &BTreeSet<String> {
        &self.runtime_lookup_only_vars
    }

    pub(super) fn seeded_runtime_const_op_indices(&self) -> &BTreeSet<usize> {
        &self.seeded_runtime_const_op_indices
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        &self.const_cache
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        &self.scalar_plan
    }

    pub(super) fn multi_return(&self) -> &WasmMultiReturnLayout {
        &self.multi_return
    }

    pub(super) fn tail_call_eligible(&self) -> bool {
        self.tail_call_eligible
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.arena_local
    }

    pub(super) fn emit_debug_local_map(&self, func_ir: &FunctionIR) {
        if std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok().as_deref()
            != Some(func_ir.name.as_str())
        {
            return;
        }
        eprintln!("WASM_DEBUG_FUNC {}", func_ir.name);
        for (idx, op) in func_ir.ops.iter().enumerate() {
            let mut mentioned: Vec<String> = Vec::new();
            if let Some(args) = &op.args {
                mentioned.extend(args.iter().cloned());
            }
            if let Some(var) = &op.var {
                mentioned.push(var.clone());
            }
            if let Some(out) = &op.out {
                mentioned.push(out.clone());
            }
            mentioned.sort();
            mentioned.dedup();
            let mapped: Vec<String> = mentioned
                .into_iter()
                .filter_map(|name| self.locals.get(&name).map(|slot| format!("{name}->{slot}")))
                .collect();
            eprintln!(
                "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?}",
                idx, op.kind, op.var, op.out, op.args, mapped
            );
        }
    }

    pub(super) fn emit_dispatch_seed_initializers(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        if !self.control_mode.needs_dispatch() {
            return;
        }
        for (_, op) in &self.seeded_runtime_const_ops {
            emit_seeded_runtime_const_op(
                backend,
                func,
                op,
                &self.locals,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
            );
        }
        // Seed dispatch locals from their first literal assignment so control-flow
        // edge threading cannot observe a raw wasm zero (0.0 bits) for an
        // otherwise integer/none local before its defining block executes.
        for (local_idx, bits) in self.const_seed_locals.iter().copied() {
            func.instruction(&Instruction::I64Const(bits));
            func.instruction(&Instruction::LocalSet(local_idx));
        }
    }

    pub(super) fn emit_entry_initializers(
        &self,
        func: &mut Function,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
    ) {
        self.const_cache.emit_init(func);
        if let Some(idx) = self.arena_local {
            emit_call(func, reloc_enabled, import_ids["arena_new"]);
            func.instruction(&Instruction::LocalSet(idx));
        }
    }

    pub(super) fn emit_implicit_return(
        &self,
        func: &mut Function,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
    ) {
        if let Some(arena_idx) = self.arena_local {
            func.instruction(&Instruction::LocalGet(arena_idx));
            emit_call(func, reloc_enabled, import_ids["arena_free"]);
        }
        self.const_cache.emit_none(func);
        func.instruction(&Instruction::End);
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

        let first = locals.ensure_literal_scratch("payload", &mut local_types, &mut local_count);
        let second = locals.ensure_literal_scratch("payload", &mut local_types, &mut local_count);
        let looked_up = locals.literal_scratch("payload");
        let maybe_lookup = locals.try_literal_scratch("payload");

        assert_eq!(first.ptr_local(), 0);
        assert_eq!(first.len_local(), 1);
        assert_eq!(second.ptr_local(), first.ptr_local());
        assert_eq!(second.len_local(), first.len_local());
        assert_eq!(looked_up.ptr_local(), first.ptr_local());
        assert_eq!(looked_up.len_local(), first.len_local());
        assert_eq!(maybe_lookup.map(|scratch| scratch.ptr_local()), Some(0));
        assert!(locals.try_literal_scratch("missing").is_none());
        assert_eq!(
            locals.name_kind("payload_ptr"),
            Some(WasmFrameLocalKind::LiteralScratchPtr)
        );
        assert_eq!(
            locals.name_kind("payload_len"),
            Some(WasmFrameLocalKind::LiteralScratchLen)
        );
        assert!(locals.is_call_retention_exempt_name("payload_ptr"));
        assert!(locals.is_call_retention_exempt_name("payload_len"));
        assert!(WasmFrameLocals::is_literal_scratch_name("payload_ptr"));
        assert!(WasmFrameLocals::is_literal_scratch_name("payload_len"));
        assert!(!WasmFrameLocals::is_literal_scratch_name("payload"));
        assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
        assert_eq!(local_count, 2);
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

        assert!(WasmFrameLocals::is_synthetic_name("__dead_sink"));
        assert!(WasmFrameLocals::is_synthetic_name("__molt_tmp0"));
        assert!(WasmFrameLocals::is_synthetic_name("__wasm_tmp0"));
        assert_eq!(
            locals.name_kind("__molt_tmp0"),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::MoltTmp0
            ))
        );
        assert_eq!(
            locals.name_kind("__wasm_tmp0"),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::WasmTmp0
            ))
        );
        assert!(locals.is_call_retention_exempt_name("__molt_tmp0"));
        assert!(locals.is_call_retention_exempt_name("__wasm_tmp0"));
        assert!(locals.is_call_retention_exempt_name("none"));
        assert!(!WasmFrameLocals::is_synthetic_name("__tmp0"));
        assert!(!locals.is_call_retention_exempt_name("__tmp0"));
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

    #[test]
    fn coalescable_value_names_exclude_frame_owned_scratch() {
        let read_vars = BTreeSet::from([
            "__tmp0".to_string(),
            "__v1".to_string(),
            "__molt_tmp0".to_string(),
            "payload_ptr".to_string(),
        ]);
        let param_set = BTreeSet::from(["__tmp_param".to_string()]);

        assert!(WasmFrameLocals::is_coalescable_value_name(
            "__tmp0", &read_vars, &param_set
        ));
        assert!(WasmFrameLocals::is_coalescable_value_name(
            "__v1", &read_vars, &param_set
        ));
        assert!(!WasmFrameLocals::is_coalescable_value_name(
            "__molt_tmp0",
            &read_vars,
            &param_set
        ));
        assert!(!WasmFrameLocals::is_coalescable_value_name(
            "payload_ptr",
            &read_vars,
            &param_set
        ));
        assert!(!WasmFrameLocals::is_coalescable_value_name(
            "__tmp_param",
            &read_vars,
            &param_set
        ));
    }
}
