use std::collections::{HashMap, HashSet};

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::RefcountBalanceRole;
use crate::tir::ops::OpCode;
use crate::tir::passes::alias_analysis::build_alias_union_find;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::balance::{is_refcount_balance_op, refcount_balance_role};
use super::facts::{build_heap_exposed_set, collect_alloc_values};

pub(super) fn eliminate_non_heap_exposed_refs(func: &mut TirFunction, stats: &mut PassStats) {
    let aliases = build_alias_union_find(func);
    let heap_exposed: HashSet<ValueId> = build_heap_exposed_set(func)
        .into_iter()
        .map(|v| aliases.root(v))
        .collect();

    let finalizer_roots: HashSet<ValueId> =
        super::super::escape_analysis::finalizer_alloc_roots(func)
            .into_iter()
            .map(|v| aliases.root(v))
            .collect();

    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = match func.blocks.get_mut(&bid) {
            Some(b) => b,
            None => continue,
        };

        let before_len = block.ops.len();
        block.ops.retain(|op| {
            if is_refcount_balance_op(op.opcode)
                && op.operands.first().is_some_and(|v| {
                    let root = aliases.root(*v);
                    !heap_exposed.contains(&root) && !finalizer_roots.contains(&root)
                })
            {
                return false;
            }
            true
        });
        stats.ops_removed += before_len - block.ops.len();
    }
}

pub(super) fn promote_unique_decref_to_free(func: &mut TirFunction, stats: &mut PassStats) {
    let heap_exposed = build_heap_exposed_set(func);
    let finalizer_roots = super::super::escape_analysis::finalizer_alloc_roots(func);
    let alloc_vals = collect_alloc_values(func);

    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = match func.blocks.get_mut(&bid) {
            Some(b) => b,
            None => continue,
        };

        let mut refcount_balance: HashMap<ValueId, i32> = HashMap::new();

        for op in &mut block.ops {
            let Some(&val) = op.operands.first() else {
                continue;
            };
            match refcount_balance_role(op.opcode) {
                RefcountBalanceRole::Increment => {
                    *refcount_balance.entry(val).or_insert(0) +=
                        RefcountBalanceRole::Increment.delta();
                }
                RefcountBalanceRole::Decrement => {
                    let balance = refcount_balance.entry(val).or_insert(0);
                    if *balance == 0
                        && alloc_vals.contains(&val)
                        && !heap_exposed.contains(&val)
                        && !finalizer_roots.contains(&val)
                    {
                        op.opcode = OpCode::Free;
                        stats.values_changed += 1;
                    } else {
                        *balance += RefcountBalanceRole::Decrement.delta();
                    }
                }
                RefcountBalanceRole::NotRefcountBalance => {}
            }
        }
    }
}
