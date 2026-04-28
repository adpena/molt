//! Perceus-style reuse analysis pass for TIR.
//!
//! Based on "Perceus: Garbage Free Reference Counting with Reuse" (Reinking
//! et al., MSR, PLDI 2021).
//!
//! When a `DecRef(x)` would free an object and the immediately following
//! allocation produces an object of compatible size, we can REUSE the memory
//! instead of freeing and reallocating.  This eliminates allocation overhead
//! in patterns like list comprehensions, functional map/filter chains, and
//! dict comprehensions.
//!
//! This pass performs the *analysis only* — it identifies reuse candidates and
//! annotates `DecRef` → `Alloc` pairs with `ReuseCandidate` metadata.  The
//! actual runtime reuse tokens (`molt_reuse_token`, `molt_reuse_alloc`) are
//! not emitted yet; that is left for a future lowering pass.
//!
//! ## Algorithm
//!
//! For each basic block:
//! 1. Scan for `DecRef(x)` ops where `x` is a heap-allocated value (produced
//!    by `Alloc`, not `StackAlloc`).
//! 2. Search forward for the next `Alloc` op, skipping only non-aliasing ops
//!    (ops that cannot observe or modify `x`'s memory).
//! 3. If the types are compatible (same allocation size class), record a
//!    `ReuseCandidate` pairing the `DecRef` and `Alloc`.
//! 4. Annotate both ops with reuse metadata attributes so downstream passes
//!    and lowering can emit the conditional reuse tokens.
//!
//! ## Size compatibility
//!
//! Two TIR types are reuse-compatible if they belong to the same allocation
//! size class.  Currently we use a conservative type-equality check; future
//! work can relax this to size-class equivalence once the runtime exposes
//! allocator size classes.

use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::type_refine;
use crate::tir::values::ValueId;

use super::PassStats;

/// A reuse candidate: a `DecRef` whose freed memory can potentially be reused
/// by a subsequent `Alloc` in the same basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseCandidate {
    /// The value being DecRef'd (the potential reuse source).
    pub decref_value: ValueId,
    /// The op index of the DecRef within its block.
    pub decref_op_idx: usize,
    /// The result value of the paired Alloc (the reuse sink).
    pub alloc_value: ValueId,
    /// The op index of the Alloc within its block.
    pub alloc_op_idx: usize,
    /// The block containing both ops.
    pub block_id: crate::tir::blocks::BlockId,
}

/// Allocation size class for reuse compatibility.
///
/// Two types are reuse-compatible iff they map to the same size class.
/// This is conservative: we only match types that are structurally identical
/// or belong to known fixed-size categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SizeClass {
    /// A specific known type with fixed allocation size.
    Typed(crate::tir::types::TirType),
    /// Dynamic/unknown size — never matches anything.
    Dynamic,
}

/// Map a TIR type to its allocation size class.
fn size_class(ty: &crate::tir::types::TirType) -> SizeClass {
    use crate::tir::types::TirType;
    match ty {
        // Unboxed scalars are not heap-allocated; shouldn't appear here but
        // classify as Dynamic to prevent false matches.
        TirType::I64 | TirType::F64 | TirType::Bool | TirType::None | TirType::Never => {
            SizeClass::Dynamic
        }
        // Reference types with known fixed headers: same outer type constructor
        // means same allocation size (elements are separately allocated).
        TirType::Str
        | TirType::Bytes
        | TirType::BigInt
        | TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Set(_) => SizeClass::Typed(ty.clone()),
        // Tuples: same arity = same size.
        TirType::Tuple(elems) => SizeClass::Typed(TirType::Tuple(
            elems.iter().map(|_| TirType::DynBox).collect(),
        )),
        // Boxed values: all NaN-boxes have the same size.
        TirType::Box(_) | TirType::DynBox => SizeClass::Typed(TirType::DynBox),
        // User classes: same class id ⇒ identical instance layout
        // (frontend's `class_info["size"]` is determined statically
        // from `field_order`).  Different class ids may have
        // different field counts and therefore different layouts —
        // only same-id instances are reuse-compatible.  Encoding the
        // class id directly in the size class delegates the
        // equality check to the existing `Eq` impl on `TirType`.
        TirType::UserClass(_) => SizeClass::Typed(ty.clone()),
        // Func, Ptr, Union — conservative, treat as unique.
        TirType::Func(_) => SizeClass::Typed(ty.clone()),
        TirType::Ptr(_) => SizeClass::Dynamic,
        TirType::Union(_) => SizeClass::Dynamic,
    }
}

/// Returns `true` if a `List(A)` and `List(B)` are reuse-compatible.
/// List headers are the same size regardless of element type.
fn lists_compatible(
    a: &crate::tir::types::TirType,
    b: &crate::tir::types::TirType,
) -> bool {
    use crate::tir::types::TirType;
    matches!((a, b), (TirType::List(_), TirType::List(_)))
}

/// Returns `true` if a `Dict(K1,V1)` and `Dict(K2,V2)` are reuse-compatible.
fn dicts_compatible(
    a: &crate::tir::types::TirType,
    b: &crate::tir::types::TirType,
) -> bool {
    use crate::tir::types::TirType;
    matches!((a, b), (TirType::Dict(_, _), TirType::Dict(_, _)))
}

/// Returns `true` if two types are reuse-compatible (same allocation size class).
fn reuse_compatible(
    a: &crate::tir::types::TirType,
    b: &crate::tir::types::TirType,
) -> bool {
    // Fast path: structural equality.
    if a == b {
        return true;
    }
    // Container types: headers are same-sized regardless of element type params.
    if lists_compatible(a, b) || dicts_compatible(a, b) {
        return true;
    }
    // Fall back to size class comparison.
    let sa = size_class(a);
    let sb = size_class(b);
    // Dynamic never matches.
    if sa == SizeClass::Dynamic || sb == SizeClass::Dynamic {
        return false;
    }
    sa == sb
}

/// Returns `true` if the op at the given index might alias with or observe
/// the memory of `val`. Conservative: any op that could read/write heap memory
/// through `val` is considered aliasing.
fn is_aliasing_op(func: &TirFunction, block_id: crate::tir::blocks::BlockId, op_idx: usize, val: ValueId) -> bool {
    let block = &func.blocks[&block_id];
    let op = &block.ops[op_idx];

    // If the op directly uses `val` as an operand, it aliases.
    if op.operands.contains(&val) {
        return true;
    }

    // Ops that can observe/modify arbitrary heap state are barriers.
    matches!(
        op.opcode,
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::StoreAttr
            | OpCode::StoreIndex
            | OpCode::Raise
            | OpCode::Yield
            | OpCode::YieldFrom
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::ClosureStore
            | OpCode::Free
    )
}

/// Analyze a TIR function for Perceus-style reuse candidates.
///
/// Returns a list of `ReuseCandidate` structs identifying `DecRef` → `Alloc`
/// pairs that can potentially be converted to reuse tokens.
pub fn analyze(func: &TirFunction) -> Vec<ReuseCandidate> {
    let type_map = type_refine::extract_type_map(func);

    // Collect all values produced by `Alloc` (not `StackAlloc` — those are
    // already on the stack and don't participate in heap reuse).
    let heap_allocs: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| {
            block.ops.iter().filter_map(|op| {
                if op.opcode == OpCode::Alloc {
                    op.results.first().copied()
                } else {
                    None
                }
            })
        })
        .collect();

    let mut candidates = Vec::new();

    // Track which Alloc ops have already been paired to prevent double-reuse.
    let mut paired_allocs: HashSet<(crate::tir::blocks::BlockId, usize)> = HashSet::new();

    // Sorted block iteration for deterministic output.
    let mut block_ids: Vec<crate::tir::blocks::BlockId> =
        func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        let ops = &block.ops;

        for (decref_idx, op) in ops.iter().enumerate() {
            // Only consider DecRef ops on heap-allocated values.
            if op.opcode != OpCode::DecRef {
                continue;
            }
            let decref_val = match op.operands.first() {
                Some(&v) if heap_allocs.contains(&v) => v,
                _ => continue,
            };

            let decref_type = match type_map.get(&decref_val) {
                Some(ty) => ty,
                None => continue,
            };

            // Search forward for the next unpaired Alloc with compatible size.
            let mut found = false;
            for alloc_idx in (decref_idx + 1)..ops.len() {
                let candidate_op = &ops[alloc_idx];

                // Check for aliasing barrier between DecRef and this op.
                if is_aliasing_op(func, bid, alloc_idx, decref_val) {
                    // Hit a barrier — stop searching for this DecRef.
                    break;
                }

                if candidate_op.opcode == OpCode::Alloc {
                    // Already paired with another DecRef? Skip.
                    if paired_allocs.contains(&(bid, alloc_idx)) {
                        continue;
                    }

                    let alloc_val = match candidate_op.results.first() {
                        Some(&v) => v,
                        None => continue,
                    };

                    let alloc_type = match type_map.get(&alloc_val) {
                        Some(ty) => ty,
                        None => continue,
                    };

                    if reuse_compatible(decref_type, alloc_type) {
                        candidates.push(ReuseCandidate {
                            decref_value: decref_val,
                            decref_op_idx: decref_idx,
                            alloc_value: alloc_val,
                            alloc_op_idx: alloc_idx,
                            block_id: bid,
                        });
                        paired_allocs.insert((bid, alloc_idx));
                        found = true;
                        break;
                    }
                }
            }
            let _ = found;
        }
    }

    candidates
}

/// Annotate `DecRef` and `Alloc` ops with reuse candidate metadata.
///
/// After this pass, paired `DecRef` ops carry a `reuse_token_id` attribute
/// and paired `Alloc` ops carry a matching `reuse_from_token` attribute.
/// Downstream lowering uses these annotations to emit conditional reuse:
///
/// - `DecRef(x) [reuse_token_id=N]` → `ReuseToken(x)`: if refcount==1,
///   return pointer for reuse; else free normally.
/// - `Alloc [reuse_from_token=N]` → `ReuseAlloc(token)`: if token is
///   non-null, reuse; else malloc fresh.
pub fn annotate(func: &mut TirFunction, candidates: &[ReuseCandidate]) -> PassStats {
    let mut stats = PassStats {
        name: "reuse_analysis",
        values_changed: 0,
        ops_removed: 0,
        ops_added: 0,
    };

    for (token_id, candidate) in candidates.iter().enumerate() {
        let block = func.blocks.get_mut(&candidate.block_id).unwrap();

        // Annotate the DecRef with the reuse token ID.
        block.ops[candidate.decref_op_idx]
            .attrs
            .insert("reuse_token_id".into(), AttrValue::Int(token_id as i64));

        // Annotate the Alloc with the matching token ID.
        block.ops[candidate.alloc_op_idx]
            .attrs
            .insert("reuse_from_token".into(), AttrValue::Int(token_id as i64));

        stats.values_changed += 1;
    }

    stats
}

/// Convenience: analyze + annotate in one step.
pub fn run(func: &mut TirFunction) -> PassStats {
    let candidates = analyze(func);
    annotate(func, &candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    /// Helper to make a simple TirOp.
    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Test 1: Basic DecRef → Alloc pattern with same type produces a reuse candidate.
    ///
    /// Pattern: Alloc(x: List[DynBox]), ..., DecRef(x), Alloc(y: List[DynBox])
    /// Expected: one ReuseCandidate pairing x → y.
    #[test]
    fn decref_alloc_same_type_produces_candidate() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_x = func.fresh_value();
        let load_result = func.fresh_value();
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Alloc x (produces a list)
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_x]));
        // Use x locally (LoadAttr — non-aliasing)
        entry.ops.push(make_op(
            OpCode::LoadAttr,
            vec![alloc_x],
            vec![load_result],
        ));
        // DecRef x
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_x], vec![]));
        // Alloc y (same block, no barrier between)
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        // Return None
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert_eq!(
            candidates.len(),
            1,
            "should find exactly one reuse candidate"
        );
        assert_eq!(candidates[0].decref_value, alloc_x);
        assert_eq!(candidates[0].alloc_value, alloc_y);
    }

    /// Test 2: A barrier (Call) between DecRef and Alloc prevents reuse.
    ///
    /// Pattern: Alloc(x), DecRef(x), Call(...), Alloc(y)
    /// Expected: no reuse candidates (Call is a barrier).
    #[test]
    fn barrier_between_decref_and_alloc_prevents_reuse() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_x = func.fresh_value();
        let call_result = func.fresh_value();
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_x]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_x], vec![]));
        // Barrier: Call can observe heap state
        entry
            .ops
            .push(make_op(OpCode::Call, vec![], vec![call_result]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert!(
            candidates.is_empty(),
            "barrier between DecRef and Alloc should prevent reuse"
        );
    }

    /// Test 3: DecRef on a StackAlloc value is NOT a reuse candidate.
    ///
    /// Only heap-allocated values (produced by Alloc, not StackAlloc) participate
    /// in reuse — stack allocs are already optimally allocated.
    #[test]
    fn stack_alloc_not_eligible_for_reuse() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let stack_val = func.fresh_value();
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // StackAlloc — not a heap alloc
        entry
            .ops
            .push(make_op(OpCode::StackAlloc, vec![], vec![stack_val]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![stack_val], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert!(
            candidates.is_empty(),
            "StackAlloc values should not be reuse candidates"
        );
    }

    /// Test 4: Annotate pass correctly tags DecRef and Alloc with reuse token IDs.
    #[test]
    fn annotate_tags_ops_with_reuse_token_ids() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_x = func.fresh_value();
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_x]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_x], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1, "should annotate one reuse pair");

        let entry = &func.blocks[&func.entry_block];
        // DecRef should have reuse_token_id=0
        let decref_op = &entry.ops[1];
        assert_eq!(decref_op.opcode, OpCode::DecRef);
        assert_eq!(
            decref_op.attrs.get("reuse_token_id"),
            Some(&AttrValue::Int(0))
        );
        // Alloc should have reuse_from_token=0
        let alloc_op = &entry.ops[2];
        assert_eq!(alloc_op.opcode, OpCode::Alloc);
        assert_eq!(
            alloc_op.attrs.get("reuse_from_token"),
            Some(&AttrValue::Int(0))
        );
    }

    /// Test 5: Multiple DecRef → Alloc pairs in the same block are all captured,
    /// and each Alloc is paired at most once.
    #[test]
    fn multiple_pairs_in_same_block() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_a = func.fresh_value();
        let alloc_b = func.fresh_value();
        let alloc_c = func.fresh_value();
        let alloc_d = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Two Alloc/DecRef/Alloc patterns back-to-back
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_a]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_b]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_a], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_c]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_b], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_d]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert_eq!(candidates.len(), 2, "should find two reuse pairs");

        // First pair: DecRef(a) → Alloc(c)
        assert_eq!(candidates[0].decref_value, alloc_a);
        assert_eq!(candidates[0].alloc_value, alloc_c);

        // Second pair: DecRef(b) → Alloc(d)
        assert_eq!(candidates[1].decref_value, alloc_b);
        assert_eq!(candidates[1].alloc_value, alloc_d);
    }

    /// Test 6: DecRef on a value not produced by Alloc (e.g., a function parameter)
    /// is not a reuse candidate.
    #[test]
    fn decref_on_parameter_not_eligible() {
        let mut func =
            TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let param = ValueId(0); // entry block argument
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![param], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert!(
            candidates.is_empty(),
            "DecRef on function parameter should not produce reuse candidate"
        );
    }

    /// Test 7: Non-aliasing ops between DecRef and Alloc are skipped correctly.
    ///
    /// Pattern: Alloc(x), DecRef(x), ConstInt, Add, Alloc(y)
    /// Expected: one reuse candidate (ConstInt and Add are non-aliasing).
    #[test]
    fn non_aliasing_ops_skipped() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_x = func.fresh_value();
        let const_val = func.fresh_value();
        let add_result = func.fresh_value();
        let alloc_y = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_x]));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_x], vec![]));
        // Non-aliasing: ConstInt, Add (neither uses alloc_x nor is a barrier)
        entry
            .ops
            .push(make_op(OpCode::ConstInt, vec![], vec![const_val]));
        entry.ops.push(make_op(
            OpCode::Add,
            vec![const_val, const_val],
            vec![add_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let candidates = analyze(&func);
        assert_eq!(
            candidates.len(),
            1,
            "non-aliasing ops should not block reuse"
        );
        assert_eq!(candidates[0].decref_value, alloc_x);
        assert_eq!(candidates[0].alloc_value, alloc_y);
    }

    /// `UserClass` size classes: same class id ⇒ identical size
    /// class (instances are layout-compatible and reuse-eligible).
    /// Different class ids ⇒ distinct size classes (different
    /// `class_info["size"]` values produce different instance
    /// payloads, so a `DecRef(Point)` cannot back a `Alloc(Line)`
    /// reuse).
    #[test]
    fn user_class_size_class_matches_on_id() {
        let point_a = TirType::UserClass("Point".into());
        let point_b = TirType::UserClass("Point".into());
        let line = TirType::UserClass("Line".into());

        // Same class ⇒ same size class ⇒ reuse-compatible.
        assert_eq!(size_class(&point_a), size_class(&point_b));
        // Different classes ⇒ distinct size classes.
        assert_ne!(size_class(&point_a), size_class(&line));
        // Both should be `Typed`, not `Dynamic` — typed classes
        // have static layouts derived from `class_info["size"]`,
        // so the reuse machinery can match on the class id.
        assert!(
            matches!(size_class(&point_a), SizeClass::Typed(_)),
            "UserClass should classify as Typed (static layout), \
             not Dynamic"
        );
    }
}
