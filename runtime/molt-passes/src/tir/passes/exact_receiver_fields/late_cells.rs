//! Grow an owned module cell block only when an exact method call consumes
//! the maker's live object. Reusable objects and documents without the
//! matching module initializer cannot change its layout.
use super::{BlockId, Maker, ValueId};
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use molt_ir::FunctionId;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::ir::{GlobalSlotRef, ModuleRegistryIR};
use molt_ir::runtime_fn_generated::RuntimeFn;
use std::collections::{BTreeMap, BTreeSet};

struct Bind {
    row: usize,
    body: usize,
    block: BlockId,
    result: ValueId,
    function_cells: u32,
    words: u32,
    added: u32,
}

#[derive(Default)]
pub(in crate::tir::passes) struct CellRequests<'a> {
    registry: Option<&'a mut ModuleRegistryIR>,
    binds: HashMap<String, Bind>,
    external: BTreeSet<usize>,
    slots: BTreeMap<Maker, GlobalSlotRef>,
}

impl<'a> CellRequests<'a> {
    pub(in crate::tir::passes) fn new(
        module: &TirModule,
        registry: Option<&'a mut ModuleRegistryIR>,
        external: &HashSet<String>,
        candidates: &super::ExactMembers,
    ) -> Self {
        let Some(registry) = registry else {
            return Self::default();
        };
        let candidates: HashSet<_> = candidates.late_modules().collect();
        if candidates.is_empty() {
            return Self::default();
        }
        let owners: HashMap<_, _> = module
            .functions
            .iter()
            .enumerate()
            .map(|(index, f)| (f.name.as_str(), index))
            .collect();
        let mut binds = HashMap::default();
        for (row, entry) in registry.rows.iter().enumerate() {
            if !candidates.contains(entry.name.as_str()) {
                continue;
            }
            let (Some(words), Some(init)) = (entry.static_cells, entry.init_symbol.as_deref())
            else {
                continue;
            };
            if !external.contains(init)
                && let Some(&body) = owners.get(init)
                && let Some((block, result, function_cells)) =
                    binding(&module.functions[body], &entry.name, words)
            {
                binds.insert(
                    entry.name.clone(),
                    Bind {
                        row,
                        body,
                        block,
                        result,
                        words,
                        function_cells,
                        added: 0,
                    },
                );
            }
        }
        Self {
            registry: Some(registry),
            binds,
            external: external
                .iter()
                .filter_map(|name| owners.get(name.as_str()).copied())
                .collect(),
            slots: BTreeMap::new(),
        }
    }

    pub(in crate::tir::passes) fn exclude_cloned_owners(&mut self, origins: &[usize]) {
        // A clone can execute in place of its origin. The maker and bind must
        // have one emitted body that every construction actually executes.
        if self.binds.is_empty() {
            return;
        }
        self.external.extend(origins.iter().copied());
        self.binds
            .retain(|_, bind| !self.external.contains(&bind.body));
    }

    pub(super) fn request(&mut self, maker: &Maker) -> Option<&GlobalSlotRef> {
        if self.external.contains(&maker.body) {
            return None;
        }
        let slot = match self.slots.entry(maker.clone()) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                let bind = self.binds.get_mut(&maker.module)?;
                let index = bind.words.checked_add(bind.added)?;
                index.checked_add(1)?;
                bind.added += 1;
                e.insert(GlobalSlotRef {
                    module: maker.module.clone(),
                    index,
                    cell: true,
                    init_once: true,
                })
            }
        };
        Some(slot)
    }

    pub(in crate::tir::passes) fn publish(
        self,
        module: &mut TirModule,
        changed: &mut BTreeSet<FunctionId>,
    ) {
        let Some(registry) = self.registry else {
            return;
        };
        for (maker, slot) in self.slots {
            let f = &mut module.functions[maker.body];
            let base = f.fresh_value();
            f.value_types.insert(base, TirType::I64);
            let ops = &mut f.blocks.get_mut(&maker.block).unwrap().ops;
            let at = ops
                .iter()
                .position(|op| op.results == [maker.value])
                .expect("admitted maker remains in its block");
            let mut address = TirOp::new(OpCode::StaticCellBase, vec![], vec![base]);
            address
                .attrs
                .insert("s_value".into(), AttrValue::Str(slot.module.clone()));
            address.inherit_source_from(&ops[at]);
            let mut store = TirOp::new(OpCode::StaticCellSet, vec![base, maker.value], vec![]);
            store.set_global_slot(Some(&slot));
            store.inherit_source_from(&ops[at]);
            // A failing maker transfers at the check without publishing its
            // result. Both new operations run on the normal continuation.
            debug_assert_eq!(ops[at + 1].opcode, OpCode::CheckException);
            ops.splice(at + 2..at + 2, [address, store]);
            changed.insert(module.ids[maker.body]);
        }
        for bind in self.binds.into_values().filter(|bind| bind.added > 0) {
            let f = &mut module.functions[bind.body];
            let count = f.fresh_value();
            f.value_types.insert(count, TirType::I64);
            let ops = &mut f.blocks.get_mut(&bind.block).unwrap().ops;
            let at = ops
                .iter()
                .position(|op| op.results == [bind.result])
                .expect("admitted cell bind remains in its block");
            // The previous constant may have other uses after CSE.
            let mut length = TirOp::new(OpCode::ConstInt, vec![], vec![count]);
            length.attrs.insert(
                "value".into(),
                AttrValue::Int(i64::from(bind.function_cells + bind.added)),
            );
            length.inherit_source_from(&ops[at]);
            ops[at].operands[3] = count;
            ops.insert(at, length);
            registry.rows[bind.row].static_cells = Some(bind.words + bind.added);
            changed.insert(module.ids[bind.body]);
        }
    }
}

/// Locate the call that binds this very module's static block. A count with
/// any other producer, or a second bind in this initializer, leaves it closed.
fn binding(f: &TirFunction, owner: &str, words: u32) -> Option<(BlockId, ValueId, u32)> {
    let mut calls = f
        .blocks
        .iter()
        .flat_map(|(id, b)| b.ops.iter().map(move |op| (*id, op)))
        .filter(|(_, op)| op.runtime == Some(RuntimeFn::StaticCellsBind));
    let (block, call) = calls.next()?;
    let ([_, base, _, count], [result]) = (&call.operands[..], &call.results[..]) else {
        return None;
    };
    if calls.next().is_some() {
        return None;
    }
    let mut module_base = false;
    let mut function_cells = None;
    for op in f.blocks.values().flat_map(|b| &b.ops) {
        if op.results == [*base] && op.opcode == OpCode::StaticCellBase {
            module_base =
                matches!(op.attrs.get("s_value"), Some(AttrValue::Str(name)) if name == owner);
        }
        if op.results == [*count]
            && op.opcode == OpCode::ConstInt
            && let Some(AttrValue::Int(value)) = op.attrs.get("value")
        {
            function_cells = u32::try_from(*value).ok().filter(|n| *n <= words);
        }
    }
    module_base.then_some((block, *result, function_cells?))
}
