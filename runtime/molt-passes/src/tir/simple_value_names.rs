use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    SimpleIrVarFieldRole, opcode_canonical_kind_table, simpleir_var_field_role_table,
};
use crate::tir::ops::AttrValue;
use crate::tir::values::ValueId;

/// One allocation authority for SimpleIR value and storage transports.
/// Authored stream spellings are provenance, not SSA identity: moving a
/// definition must never overwrite a different, still-live ValueId.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleValueNames {
    value_names: HashMap<ValueId, String>,
    source_names: HashMap<ValueId, String>,
    ambiguous_source_names: HashSet<String>,
    local_slots: HashMap<String, String>,
    block_arg_slots: HashMap<(BlockId, usize), String>,
    reserved_names: HashSet<String>,
}

impl SimpleValueNames {
    pub fn for_function(func: &TirFunction) -> Self {
        let mut names = Self::default();
        let mut occupied = HashSet::new();
        let mut values = Vec::new();
        let mut slots = BTreeSet::new();
        let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
        blocks.sort_unstable_by_key(|id| id.0);

        // ABI parameter names are fixed. Mutable locals of the same spelling
        // receive separate storage below; no entry snapshot or copy shim is
        // needed to keep the incoming parameter value alive across a rebind.
        if let Some(entry) = func.blocks.get(&func.entry_block) {
            for (index, arg) in entry.args.iter().enumerate() {
                if let Some(name) = func.param_names.get(index) {
                    assert!(
                        occupied.insert(name.clone()),
                        "duplicate ABI parameter name: {name}"
                    );
                    names.value_names.insert(arg.id, name.clone());
                    names.source_names.insert(arg.id, name.clone());
                }
            }
        }
        for bid in &blocks {
            let block = &func.blocks[bid];
            values.extend(block.args.iter().map(|arg| arg.id));
            for op in &block.ops {
                values.extend(op.results.iter().copied());
                for (index, &result) in op.results.iter().enumerate() {
                    let source = op
                        .attrs
                        .get(&format!("_simple_result_{index}"))
                        .or_else(|| {
                            (op.results.len() == 1)
                                .then(|| op.attrs.get("_simple_out"))
                                .flatten()
                        });
                    if let Some(AttrValue::Str(name)) = source
                        && !name.is_empty()
                        && name != "none"
                    {
                        names.source_names.insert(result, name.clone());
                    }
                }
                let kind = match op.attrs.get("_original_kind") {
                    Some(AttrValue::Str(kind)) => kind.as_str(),
                    _ => opcode_canonical_kind_table(op.opcode),
                };
                if simpleir_var_field_role_table(kind) == SimpleIrVarFieldRole::Definition
                    && let Some(AttrValue::Str(slot)) = op.attrs.get("_var")
                {
                    slots.insert(slot.clone());
                }
            }
        }
        values.sort_unstable_by_key(|id| id.0);
        values.dedup();

        // Reserve authored spellings before minting any suffix, so allocation
        // cannot steal a later producer's requested name. Source metadata is
        // kept separately even when its emitted value name must change.
        let mut reserved = occupied.clone();
        reserved.extend(names.source_names.values().cloned());
        reserved.extend(slots.iter().cloned());
        let mut source_owners = HashMap::new();
        for (&value, source) in &names.source_names {
            if source_owners.insert(source, value).is_some() {
                names.ambiguous_source_names.insert(source.clone());
            }
        }
        for slot in slots {
            let transport = if occupied.insert(slot.clone()) {
                slot.clone()
            } else {
                Self::allocate_fresh(&format!("_slot_{slot}"), &mut reserved)
            };
            occupied.insert(transport.clone());
            names.local_slots.insert(slot, transport);
        }
        for bid in blocks {
            for index in 0..func.blocks[&bid].args.len() {
                let preferred = Self::canonical_block_arg_slot(bid, index);
                let transport = if reserved.insert(preferred.clone()) {
                    preferred
                } else {
                    Self::allocate_fresh(&preferred, &mut reserved)
                };
                occupied.insert(transport.clone());
                names.block_arg_slots.insert((bid, index), transport);
            }
        }
        for id in values {
            if names.value_names.contains_key(&id) {
                continue;
            }
            let transport = match names.source_names.get(&id) {
                Some(source) if occupied.insert(source.clone()) => source.clone(),
                _ => {
                    let canonical = Self::canonical_value_name(id);
                    if reserved.insert(canonical.clone()) {
                        canonical
                    } else {
                        Self::allocate_fresh(&canonical, &mut reserved)
                    }
                }
            };
            occupied.insert(transport.clone());
            reserved.insert(transport.clone());
            names.value_names.insert(id, transport);
        }
        names.reserved_names = reserved;
        names
    }

    fn allocate_fresh(base: &str, reserved: &mut HashSet<String>) -> String {
        for suffix in 0u64.. {
            let candidate = format!("{base}_c{suffix}");
            if reserved.insert(candidate.clone()) {
                return candidate;
            }
        }
        unreachable!("SimpleIR name space exhausted")
    }

    pub fn value_name(&self, id: ValueId) -> String {
        self.value_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Self::canonical_value_name(id))
    }

    /// Authored input-stream identity, never a synthesized transport name.
    /// Consumers projecting facts back to that input must reject ambiguous
    /// source names rather than select one of their distinct SSA definitions.
    pub fn source_value_name(&self, id: ValueId) -> Option<&str> {
        self.source_names.get(&id).map(String::as_str)
    }

    pub fn ambiguous_source_names(&self) -> impl Iterator<Item = &str> {
        self.ambiguous_source_names.iter().map(String::as_str)
    }

    pub fn source_name_is_ambiguous(&self, name: &str) -> bool {
        self.ambiguous_source_names.contains(name)
    }

    /// The input stream's name when known, otherwise a synthetic value name.
    /// This is only a fact-projection key; code emission uses value_name.
    pub fn source_or_value_name(&self, id: ValueId) -> String {
        self.source_value_name(id)
            .map(str::to_string)
            .unwrap_or_else(|| self.value_name(id))
    }

    pub fn local_slot(&self, source: &str) -> String {
        self.local_slots
            .get(source)
            .cloned()
            .unwrap_or_else(|| source.to_string())
    }

    /// Allocate an emission-only value in the same namespace as authored
    /// values, ABI parameters, and storage. These values have no source fact.
    pub fn fresh_temporary(&mut self, preferred: &str) -> String {
        if self.reserved_names.insert(preferred.to_string()) {
            preferred.to_string()
        } else {
            Self::allocate_fresh(preferred, &mut self.reserved_names)
        }
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

pub fn set_value_names(names: SimpleValueNames) {
    VALUE_NAMES.with(|slot| *slot.borrow_mut() = names);
}

pub fn reset_value_names() {
    set_value_names(SimpleValueNames::default());
}

pub fn value_var(id: ValueId) -> String {
    VALUE_NAMES.with(|names| names.borrow().value_name(id))
}

pub fn local_slot_var(source: &str) -> String {
    VALUE_NAMES.with(|names| names.borrow().local_slot(source))
}

pub fn temporary_var(preferred: &str) -> String {
    VALUE_NAMES.with(|names| names.borrow_mut().fresh_temporary(preferred))
}
