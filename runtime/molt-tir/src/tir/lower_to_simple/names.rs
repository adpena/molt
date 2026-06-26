use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::AttrValue;
use crate::tir::values::ValueId;

/// Canonical naming bridge from TIR SSA values and block arguments to the
/// legacy SimpleIR variable namespace consumed by existing backends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleValueNames {
    value_overrides: HashMap<ValueId, String>,
    block_arg_slots: HashMap<(BlockId, usize), String>,
}

impl SimpleValueNames {
    pub fn for_function(func: &TirFunction) -> Self {
        let mut names = Self::default();

        // ── Phase 1: collect the EXPLICIT-override names ────────────────────
        // Entry-param names and `_simple_out` / `_simple_result_N` stream names
        // are authoritative: they are the SimpleIR identities downstream
        // consumers (the scalar representation plan, store_var/load_var edges)
        // key on. We assign them first and reserve every name they consume.
        //
        // A value with an explicit override keeps it verbatim. Two different
        // overrides that name the SAME string would already be a frontend bug;
        // we do not attempt to rename overrides (they are the contract). We DO
        // protect *canonical* fallbacks from colliding with an override name
        // belonging to a different value — the re-lift hazard documented on
        // `has_override`: the TIR inliner mints fresh ValueIds, so a value's
        // canonical `_v{id}` can land on a string a *different* value already
        // claimed via `_simple_out` (carried verbatim from the pre-lift stream).
        // Without this protection both values resolve to one SimpleIR name and
        // `rewrite_copy_aliases` conflates them — a silent wrong-value miscompile
        // (observed: a module-scope guarded property merge reading the cold slow
        // path on the hot fast edge).
        if let Some(entry_block) = func.blocks.get(&func.entry_block) {
            for (idx, arg) in entry_block.args.iter().enumerate() {
                if let Some(name) = func.param_names.get(idx) {
                    names.value_overrides.insert(arg.id, name.clone());
                }
            }
        }
        for (bid, block) in &func.blocks {
            for index in 0..block.args.len() {
                names
                    .block_arg_slots
                    .insert((*bid, index), Self::canonical_block_arg_slot(*bid, index));
            }
            for op in &block.ops {
                for (index, result) in op.results.iter().enumerate() {
                    let key = format!("_simple_result_{index}");
                    if let Some(AttrValue::Str(name)) = op.attrs.get(&key) {
                        names.value_overrides.insert(*result, name.clone());
                    }
                }
                if op.results.len() == 1
                    && let Some(result) = op.results.first()
                    && let Some(AttrValue::Str(name)) = op.attrs.get("_simple_out")
                {
                    names.value_overrides.insert(*result, name.clone());
                }
            }
        }

        // ── Phase 2: resolve canonical-name collisions ──────────────────────
        // Reserve every name already consumed: all explicit overrides. Then,
        // for every value WITHOUT an override, check whether its canonical
        // `_v{id}` name collides with a reserved name (an override on a
        // different value, or a canonical name already handed to another value).
        // On collision, mint a fresh deterministic name and record it as an
        // override so `value_name` returns it. Values are visited in ascending
        // ValueId order so the assignment is stable across builds.
        let mut reserved: HashSet<String> = names.value_overrides.values().cloned().collect();

        let mut all_values: Vec<ValueId> = Vec::new();
        if let Some(entry_block) = func.blocks.get(&func.entry_block) {
            for arg in &entry_block.args {
                all_values.push(arg.id);
            }
        }
        for block in func.blocks.values() {
            for arg in &block.args {
                all_values.push(arg.id);
            }
            for op in &block.ops {
                for result in &op.results {
                    all_values.push(*result);
                }
            }
        }
        all_values.sort_unstable_by_key(|v| v.0);
        all_values.dedup();

        for id in all_values {
            if names.value_overrides.contains_key(&id) {
                // Already has an authoritative name; it is reserved.
                continue;
            }
            let canonical = Self::canonical_value_name(id);
            if !reserved.contains(&canonical) {
                // Canonical name is free — claim it (reserve so a later value's
                // canonical or fresh name cannot re-collide).
                reserved.insert(canonical);
                continue;
            }
            // Collision: this value's canonical name belongs to a different
            // value (via override). Mint a fresh, collision-free name and pin
            // it as an override so `value_name` returns it deterministically.
            let mut suffix = 0u32;
            let fresh = loop {
                let candidate = format!("_v{}_c{}", id.0, suffix);
                if !reserved.contains(&candidate) {
                    break candidate;
                }
                suffix += 1;
            };
            reserved.insert(fresh.clone());
            names.value_overrides.insert(id, fresh);
        }

        names
    }

    pub fn value_name(&self, id: ValueId) -> String {
        self.value_overrides
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Self::canonical_value_name(id))
    }

    /// True if `id` carries an EXPLICIT SimpleIR name (a `_simple_out` /
    /// `_simple_result_N` override) — the stream's source of truth — rather
    /// than a synthetic canonical fallback (`_v{N}` / `_bb{N}_arg{I}`).
    ///
    /// Name-keyed consumers (the scalar representation plan) treat
    /// explicit-name facts as authoritative: a re-lift renumbers ValueIds, so
    /// a canonical fallback name can COLLIDE with a different value's
    /// explicit stream name; the explicit fact must win, not conflict out.
    pub fn has_override(&self, id: ValueId) -> bool {
        self.value_overrides.contains_key(&id)
    }

    pub fn block_arg_slot(&self, block: BlockId, index: usize) -> String {
        self.block_arg_slots
            .get(&(block, index))
            .cloned()
            .unwrap_or_else(|| Self::canonical_block_arg_slot(block, index))
    }

    pub fn block_arg_slots(&self, block: BlockId, arity: usize) -> Vec<String> {
        (0..arity)
            .map(|index| self.block_arg_slot(block, index))
            .collect()
    }

    pub fn canonical_value_name(id: ValueId) -> String {
        format!("_v{}", id.0)
    }

    pub fn canonical_block_arg_slot(block: BlockId, index: usize) -> String {
        format!("_bb{}_arg{}", block.0, index)
    }
}

thread_local! {
    static VALUE_NAMES: RefCell<SimpleValueNames> = RefCell::new(SimpleValueNames::default());
}

pub(super) fn set_value_names(names: SimpleValueNames) {
    VALUE_NAMES.with(|slot| *slot.borrow_mut() = names);
}

pub(super) fn reset_value_names() {
    set_value_names(SimpleValueNames::default());
}

/// Synthesise a SimpleIR variable name from a ValueId.
pub(super) fn value_var(id: ValueId) -> String {
    VALUE_NAMES.with(|names| names.borrow().value_name(id))
}
