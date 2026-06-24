//! Lower typed TIR into representation-aware LIR.

use std::collections::HashMap;

use super::blocks::{Terminator, TirBlock};
use super::dominators;
use super::function::TirFunction;
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::{AttrDict, AttrValue, OpCode, TirOp};
use super::type_refine::{extract_type_map, refine_types};
use super::types::TirType;
use super::values::{TirValue, ValueId};
use crate::repr::Repr;
use crate::tir::op_kinds_generated::{
    opcode_requires_i64_overflow_box_dispatch_table, opcode_requires_i64_zero_divisor_guard_table,
    opcode_supports_i64_checked_overflow_triple_table,
};

/// The proven per-`ValueId` representation override (the value-keyed source of
/// truth produced by `representation_plan::repr_by_value_for`). The WASM/LIR
/// codegen path threads `Some(map)` so `LirRepr::I64` is assigned **only** to
/// proven `RawI64Safe` integers; an unproven `int` (`MaybeBigInt`) lowers to
/// `DynBox` and uses the boxed BigInt-correct runtime path (typed-IR Phase 1).
pub type ReprOverride<'a> = Option<&'a HashMap<ValueId, Repr>>;

/// Derive the backend-facing [`LirRepr`] for a value from the proven [`Repr`]
/// override when present, falling back to the type floor [`LirRepr::for_type`]
/// when no override is supplied or the value is absent from the map.
///
/// The ONLY behavior change versus a bare `for_type(ty)` is: a non-`RawI64Safe`
/// `I64` value derives `DynBox` instead of `I64` — the Phase-1 fix that stops
/// WASM treating an unproven (possibly heap-BigInt) `int` as a raw inline i64.
/// `Bool`/`FloatUnboxed` are floored into `repr_by_value` by Phase 0's
/// `default_for`, so they are present and map back to `Bool1`/`F64`.
fn lir_repr_from_repr(repr: ReprOverride<'_>, id: ValueId, ty: &TirType) -> LirRepr {
    match repr.and_then(|map| map.get(&id)) {
        Some(Repr::RawI64Safe) => LirRepr::I64,
        Some(Repr::Bool) => LirRepr::Bool1,
        Some(Repr::FloatUnboxed) => LirRepr::F64,
        // `MaybeBigInt` (unproven int), `DynBox`, `Never`: the universal NaN-box
        // carrier — no raw machine op is sound, so it lowers to `DynBox` and the
        // boxed runtime helpers (BigInt-correct) handle it.
        Some(Repr::MaybeBigInt) | Some(Repr::DynBox) | Some(Repr::Never) => LirRepr::DynBox,
        None => LirRepr::for_type(ty),
    }
}

pub fn lower_function_to_lir(func: &TirFunction, repr: ReprOverride<'_>) -> LirFunction {
    lower_function_to_lir_with_inline_proof(func, repr, None)
}

/// [`lower_function_to_lir`] with the value-range proof threaded in.
///
/// `inline_proof` is the ValueRange analysis computed on this EXACT `func`
/// (ValueIds must line up). It gates the checked-i64 triple: the triple's
/// raw `I64Add` + inline-range check + inline-boxed overflow arm is only
/// sound when operands and result are PROVEN inside the 47-bit window —
/// `Repr::RawI64Safe` alone no longer implies that (it is a FULL-RANGE
/// carrier contract; the overflow_peel accumulators are genuinely
/// unbounded). Production value-keyed callers (the WASM fast lane) must
/// supply the proof; `None` keeps the legacy type/repr-gated behavior for
/// fact extraction and hand-built-repr tests.
pub fn lower_function_to_lir_with_inline_proof(
    func: &TirFunction,
    repr: ReprOverride<'_>,
    inline_proof: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> LirFunction {
    let mut refined = func.clone();
    refine_types(&mut refined);
    canonicalize_non_executable_blocks(&mut refined);
    let type_map = extract_type_map(&refined);
    let mut allocator = ValueIdAllocator::new(refined.next_value);

    let mut block_ids: Vec<_> = refined.blocks.keys().copied().collect();
    block_ids.sort_by_key(|bid| bid.0);
    let blocks = block_ids
        .into_iter()
        .map(|bid| {
            let block = refined
                .blocks
                .get(&bid)
                .expect("sorted block id must exist");
            (
                bid,
                lower_block(
                    block,
                    &refined,
                    &type_map,
                    &mut allocator,
                    repr,
                    inline_proof,
                ),
            )
        })
        .collect();
    let entry_param_types = refined
        .blocks
        .get(&refined.entry_block)
        .map(|block| {
            block
                .args
                .iter()
                .map(|arg| arg.ty.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut param_names = refined.param_names.clone();
    if param_names.len() != entry_param_types.len() {
        param_names = (0..entry_param_types.len())
            .map(|idx| format!("p{idx}"))
            .collect();
    }
    let return_types = lir_return_types(&refined);

    let label_id_map = refined.label_id_map.clone();
    LirFunction {
        name: refined.name,
        param_names,
        param_types: entry_param_types,
        return_types,
        blocks,
        entry_block: refined.entry_block,
        label_id_map,
    }
}

fn canonicalize_non_executable_blocks(func: &mut TirFunction) {
    let executable = dominators::executable_reachable_blocks(func);
    for (bid, block) in &mut func.blocks {
        if executable.contains(bid) {
            continue;
        }
        block.ops.clear();
        block.terminator = Terminator::Unreachable;
    }
}

fn lir_return_types(func: &TirFunction) -> Vec<TirType> {
    let mut arities = func
        .blocks
        .values()
        .filter_map(|block| match &block.terminator {
            Terminator::Return { values } => Some(values.len()),
            _ => None,
        })
        .collect::<Vec<_>>();
    arities.sort_unstable();
    arities.dedup();
    match arities.as_slice() {
        [] => Vec::new(),
        [0] => Vec::new(),
        [1] => vec![func.return_type.clone()],
        _ => match &func.return_type {
            TirType::Tuple(items) if items.len() == *arities.iter().max().unwrap_or(&0) => {
                items.clone()
            }
            other => vec![other.clone()],
        },
    }
}

pub fn lower_block_args(args: &[TirValue], repr: ReprOverride<'_>) -> Vec<LirValue> {
    args.iter()
        .map(|arg| LirValue {
            id: arg.id,
            repr: lir_repr_from_repr(repr, arg.id, &arg.ty),
            ty: arg.ty.clone(),
        })
        .collect()
}

fn lower_block(
    block: &TirBlock,
    func: &TirFunction,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    repr: ReprOverride<'_>,
    inline_proof: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> LirBlock {
    let mut ops = lower_block_ops(
        block.ops.as_slice(),
        type_map,
        allocator,
        repr,
        inline_proof,
    );
    let terminator = lower_terminator(&block.terminator, func, type_map, allocator, &mut ops, repr);
    LirBlock {
        id: block.id,
        args: lower_block_args(&block.args, repr),
        ops,
        terminator,
    }
}

fn lower_block_ops(
    ops: &[TirOp],
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    repr: ReprOverride<'_>,
    inline_proof: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> Vec<LirOp> {
    ops.iter()
        .map(|op| lower_op(op, type_map, allocator, repr, inline_proof))
        .collect()
}

fn lower_op(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    repr: ReprOverride<'_>,
    inline_proof: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> LirOp {
    if lowers_to_checked_i64_arithmetic(op, type_map, repr, inline_proof) {
        return lower_checked_i64_arithmetic(op, type_map, allocator, repr);
    }
    // Divisor-zero safety for the raw-i64 division family. The WASM I64 lane
    // emits a bare `i64.div_s`/`i64.rem_s`, which TRAPS on a zero divisor
    // (CPython raises `ZeroDivisionError`). A zero divisor is only safe in the
    // raw lane when value-range analysis PROVES the divisor non-zero; otherwise
    // force the boxed runtime dispatch (`molt_floordiv`/`molt_mod`/`molt_div`),
    // which raises correctly. This is the WASM analogue of the native inline
    // zero-guard and the LLVM `emit_i64_divrem_zero_guarded` fast/slow split —
    // decided HERE, where the value-range proof lives, not in the emitter.
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode) && op.operands.len() >= 2 {
        let divisor = op.operands[1];
        let divisor_nonzero = inline_proof.is_some_and(|vr| vr.range_of(divisor).proves_nonzero());
        if !divisor_nonzero {
            let mut tir_op = op.clone();
            tir_op
                .attrs
                .insert("lir.boxed_dispatch".to_string(), AttrValue::Bool(true));
            let result_values = tir_op
                .results
                .iter()
                .map(|result_id| lir_value_from_type_map(*result_id, type_map, repr))
                .collect();
            return LirOp {
                tir_op,
                result_values,
            };
        }
    }
    // Arithmetic on raw-i64 carriers WITHOUT the inline-window proof: a bare
    // machine op could silently wrap at 2^63 (`RawI64Safe` is a FULL-RANGE
    // carrier contract — CheckedAdd sums / overflow_peel accumulators are
    // unbounded). Such ops are marked for the BOXED runtime dispatch; only
    // proof-carrying ops take the checked triple above, and only
    // proven-result raw ops may use a bare machine instruction. The decision
    // is made HERE (where the value-range proof lives), not in the wasm
    // emitter (which only sees reprs).
    if opcode_requires_i64_overflow_box_dispatch_table(op.opcode)
        && let Some(map) = repr
        && op
            .operands
            .iter()
            .any(|id| matches!(map.get(id), Some(Repr::RawI64Safe)))
    {
        let proven = |id: &ValueId| inline_proof.is_some_and(|vr| vr.fits_inline_int47(*id));
        let all_proven = op.operands.iter().all(proven) && op.results.iter().all(proven);
        if !all_proven {
            let mut tir_op = op.clone();
            tir_op
                .attrs
                .insert("lir.boxed_dispatch".to_string(), AttrValue::Bool(true));
            let result_values = tir_op
                .results
                .iter()
                .map(|result_id| lir_value_from_type_map(*result_id, type_map, repr))
                .collect();
            return LirOp {
                tir_op,
                result_values,
            };
        }
    }
    if op.opcode == OpCode::BoxVal {
        return lower_box_op(op, type_map);
    }
    if op.opcode == OpCode::UnboxVal {
        return lower_unbox_op(op, type_map, repr);
    }
    if matches!(
        op.opcode,
        OpCode::ObjectNewBound | OpCode::ObjectNewBoundStack
    ) {
        return lower_object_new_bound_op(op, type_map);
    }

    LirOp {
        tir_op: op.clone(),
        result_values: op
            .results
            .iter()
            .map(|result_id| lir_value_from_type_map(*result_id, type_map, repr))
            .collect(),
    }
}

fn lowers_to_checked_i64_arithmetic(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    repr: ReprOverride<'_>,
    inline_proof: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> bool {
    let type_eligible = opcode_supports_i64_checked_overflow_triple_table(op.opcode)
        && op.results.len() == 1
        && op.operands.len() == 2
        && op
            .operands
            .iter()
            .all(|operand| matches!(type_map.get(operand), Some(TirType::I64)))
        && matches!(type_map.get(&op.results[0]), Some(TirType::I64));
    if !type_eligible {
        return false;
    }
    // When a proven `Repr` override is supplied (WASM/LIR codegen path), the
    // checked-i64 triple (raw `I64Add` + inline-bounds overflow box) is sound
    // ONLY when every operand and the result is PROVEN inside the 47-bit
    // inline window. `Repr::RawI64Safe` alone is NOT that proof: it is a
    // FULL-RANGE i64 carrier contract (CheckedAdd sums and overflow_peel
    // accumulators are genuinely unbounded), and a raw `I64Add` over
    // full-range operands could silently wrap at 2^63 while the triple's
    // overflow arm inline-boxes operands assuming the 47-bit window. So with
    // a repr override the gate requires the VALUE-RANGE proof
    // (`fits_inline_int47`) on operands AND result; a production caller that
    // supplies a repr override without a proof gets NO triples (falls to the
    // boxed runtime path — sound, never fast-but-wrong). A `MaybeBigInt`
    // operand must likewise take the generic boxed path. Without an override
    // (fact extraction / native / tests) the gate stays type-only so those
    // callers are byte-identical.
    match repr {
        None => true,
        Some(map) => {
            let proven_repr = |id: &ValueId| matches!(map.get(id), Some(Repr::RawI64Safe));
            let proven_inline =
                |id: &ValueId| inline_proof.is_some_and(|vr| vr.fits_inline_int47(*id));
            op.operands.iter().all(proven_repr)
                && proven_repr(&op.results[0])
                && op.operands.iter().all(proven_inline)
                && proven_inline(&op.results[0])
        }
    }
}

fn lower_checked_i64_arithmetic(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    repr: ReprOverride<'_>,
) -> LirOp {
    let mut tir_op = op.clone();
    let overflow_box = allocator.fresh();
    let overflow_flag = allocator.fresh();
    tir_op.results = vec![op.results[0], overflow_box, overflow_flag];
    tir_op
        .attrs
        .insert("lir.checked_overflow".to_string(), AttrValue::Bool(true));

    let mut result_values = vec![lir_value_from_type_map(op.results[0], type_map, repr)];
    result_values.push(LirValue {
        id: overflow_box,
        ty: TirType::DynBox,
        repr: LirRepr::DynBox,
    });
    result_values.push(LirValue {
        id: overflow_flag,
        ty: TirType::Bool,
        repr: LirRepr::Bool1,
    });

    LirOp {
        tir_op,
        result_values,
    }
}

fn lower_object_new_bound_op(op: &TirOp, type_map: &HashMap<ValueId, TirType>) -> LirOp {
    let stack_ref_eligible = op.opcode == OpCode::ObjectNewBoundStack;
    LirOp {
        tir_op: op.clone(),
        result_values: op
            .results
            .iter()
            .map(|result_id| {
                let ty = type_map.get(result_id).cloned().unwrap_or(TirType::DynBox);
                let repr = if stack_ref_eligible && matches!(ty, TirType::UserClass(_)) {
                    LirRepr::Ref64
                } else {
                    LirRepr::DynBox
                };
                LirValue {
                    id: *result_id,
                    ty,
                    repr,
                }
            })
            .collect(),
    }
}

fn lower_box_op(op: &TirOp, type_map: &HashMap<ValueId, TirType>) -> LirOp {
    let operand_ty = op
        .operands
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or(TirType::DynBox);
    let result_ty = op
        .results
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or_else(|| TirType::Box(Box::new(operand_ty)));
    let result_id = op.results[0];
    LirOp {
        tir_op: op.clone(),
        result_values: vec![LirValue {
            id: result_id,
            ty: result_ty,
            repr: LirRepr::DynBox,
        }],
    }
}

fn lower_unbox_op(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    repr: ReprOverride<'_>,
) -> LirOp {
    let operand_ty = op
        .operands
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or(TirType::DynBox);
    let result_ty = op
        .results
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or_else(|| match operand_ty {
            TirType::Box(inner) => inner.as_ref().clone(),
            _ => TirType::DynBox,
        });
    let result_id = op.results[0];
    LirOp {
        tir_op: op.clone(),
        result_values: vec![LirValue {
            id: result_id,
            repr: lir_repr_from_repr(repr, result_id, &result_ty),
            ty: result_ty,
        }],
    }
}

fn lir_value_from_type_map(
    id: ValueId,
    type_map: &HashMap<ValueId, TirType>,
    repr: ReprOverride<'_>,
) -> LirValue {
    let ty = type_map.get(&id).cloned().unwrap_or(TirType::DynBox);
    LirValue {
        id,
        repr: lir_repr_from_repr(repr, id, &ty),
        ty,
    }
}

fn lower_terminator(
    terminator: &Terminator,
    func: &TirFunction,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
    repr: ReprOverride<'_>,
) -> LirTerminator {
    match terminator {
        Terminator::Branch { target, args } => LirTerminator::Branch {
            target: *target,
            args: lower_branch_args(*target, args, func, type_map, allocator, ops, repr),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => LirTerminator::CondBranch {
            cond: materialize_branch_condition(*cond, type_map, allocator, ops),
            then_block: *then_block,
            then_args: lower_branch_args(
                *then_block,
                then_args,
                func,
                type_map,
                allocator,
                ops,
                repr,
            ),
            else_block: *else_block,
            else_args: lower_branch_args(
                *else_block,
                else_args,
                func,
                type_map,
                allocator,
                ops,
                repr,
            ),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => LirTerminator::Switch {
            value: *value,
            cases: cases.clone(),
            default: *default,
            default_args: lower_branch_args(
                *default,
                default_args,
                func,
                type_map,
                allocator,
                ops,
                repr,
            ),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => LirTerminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(state, target, args)| {
                    (
                        *state,
                        *target,
                        lower_branch_args(*target, args, func, type_map, allocator, ops, repr),
                    )
                })
                .collect(),
            default: *default,
            default_args: lower_branch_args(
                *default,
                default_args,
                func,
                type_map,
                allocator,
                ops,
                repr,
            ),
        },
        Terminator::Return { values } => LirTerminator::Return {
            values: lower_return_values(values, func, type_map, allocator, ops, repr),
        },
        Terminator::Unreachable => LirTerminator::Unreachable,
    }
}

fn lower_branch_args(
    target: super::blocks::BlockId,
    args: &[ValueId],
    func: &TirFunction,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
    repr: ReprOverride<'_>,
) -> Vec<ValueId> {
    // The target block's arguments carry both their declared type and (under an
    // override) their proven `Repr`; the materialize coercion compares the
    // source's `LirRepr` against the *target block-arg's* `LirRepr` so a
    // `MaybeBigInt`→`MaybeBigInt` phi edge (both `DynBox`) inserts no spurious
    // box/unbox.
    let expected: Vec<(TirType, Option<ValueId>)> = func
        .blocks
        .get(&target)
        .map(|block| {
            block
                .args
                .iter()
                .map(|arg| (arg.ty.clone(), Some(arg.id)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    args.iter()
        .enumerate()
        .map(|(idx, value_id)| {
            let (expected_ty, expected_id) = expected
                .get(idx)
                .cloned()
                .unwrap_or((TirType::DynBox, None));
            materialize_value_for_type(
                *value_id,
                expected_ty,
                expected_id,
                type_map,
                allocator,
                ops,
                repr,
            )
        })
        .collect()
}

fn lower_return_values(
    values: &[ValueId],
    func: &TirFunction,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
    repr: ReprOverride<'_>,
) -> Vec<ValueId> {
    // The function return surface has no SSA `ValueId` for its slots, so the
    // expected `LirRepr` is the type-floor of the return ABI type (`expected_id`
    // is `None`). The actual operand's `Repr` still comes from the override.
    let expected_types = lir_return_types(func);
    if values.is_empty() && !expected_types.is_empty() {
        return expected_types
            .iter()
            .cloned()
            .map(|expected_ty| {
                let none_id = allocator.fresh();
                ops.push(LirOp {
                    tir_op: TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: OpCode::ConstNone,
                        operands: vec![],
                        results: vec![none_id],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                    result_values: vec![LirValue {
                        id: none_id,
                        ty: TirType::None,
                        repr: LirRepr::DynBox,
                    }],
                });
                materialize_value_for_type(
                    none_id,
                    expected_ty,
                    None,
                    type_map,
                    allocator,
                    ops,
                    repr,
                )
            })
            .collect();
    }
    values
        .iter()
        .enumerate()
        .map(|(idx, value_id)| {
            let expected_ty = expected_types.get(idx).cloned().unwrap_or(TirType::DynBox);
            materialize_value_for_type(*value_id, expected_ty, None, type_map, allocator, ops, repr)
        })
        .collect()
}

fn materialize_value_for_type(
    value_id: ValueId,
    expected_ty: TirType,
    expected_id: Option<ValueId>,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
    repr: ReprOverride<'_>,
) -> ValueId {
    let actual_ty = type_map.get(&value_id).cloned().unwrap_or(TirType::DynBox);
    // The box/unbox decision is a *representation* coercion, so it must compare
    // `LirRepr`, not `TirType`. Under an override, two `TirType::I64` values can
    // carry different reprs (a proven `RawI64Safe`→`I64` source feeding an
    // unproven `MaybeBigInt`→`DynBox` slot still needs a box), so the type
    // short-circuit is only sound when the reprs also match.
    let actual_repr = lir_repr_from_repr(repr, value_id, &actual_ty);
    let expected_repr = match expected_id {
        Some(id) => lir_repr_from_repr(repr, id, &expected_ty),
        None => LirRepr::for_type(&expected_ty),
    };
    if expected_repr == actual_repr {
        return value_id;
    }
    if expected_repr == LirRepr::DynBox && actual_repr != LirRepr::DynBox {
        let boxed_id = allocator.fresh();
        ops.push(LirOp {
            tir_op: TirOp {
                dialect: super::ops::Dialect::Molt,
                opcode: OpCode::BoxVal,
                operands: vec![value_id],
                results: vec![boxed_id],
                attrs: AttrDict::new(),
                source_span: None,
            },
            result_values: vec![LirValue {
                id: boxed_id,
                ty: expected_ty,
                repr: LirRepr::DynBox,
            }],
        });
        return boxed_id;
    }
    // The reverse coercion (`DynBox` source → a non-`DynBox` slot, i.e. an
    // unbox) cannot arise for a proven phi: `compute_overflow_safe_values` raises
    // a block argument to `RawI64Safe` only when *every* incoming edge value is
    // `RawI64Safe`, so a `DynBox` (`MaybeBigInt`) source never feeds an `I64`
    // (`RawI64Safe`) slot. On the `None` path this preserves the prior behavior
    // exactly (it never inserted an unbox here either).
    value_id
}

fn materialize_branch_condition(
    cond: ValueId,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
) -> ValueId {
    if matches!(type_map.get(&cond), Some(TirType::Bool)) {
        return cond;
    }

    let result_id = allocator.fresh();
    let mut attrs = AttrDict::new();
    attrs.insert(
        "callee".to_string(),
        AttrValue::Str("molt_is_truthy".to_string()),
    );
    attrs.insert("lir.truthy_cond".to_string(), AttrValue::Bool(true));
    ops.push(LirOp {
        tir_op: TirOp {
            dialect: super::ops::Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![cond],
            results: vec![result_id],
            attrs,
            source_span: None,
        },
        result_values: vec![LirValue {
            id: result_id,
            ty: TirType::Bool,
            repr: LirRepr::Bool1,
        }],
    });
    result_id
}

struct ValueIdAllocator {
    next: u32,
}

impl ValueIdAllocator {
    fn new(next: u32) -> Self {
        Self { next }
    }

    fn fresh(&mut self) -> ValueId {
        let id = ValueId(self.next);
        self.next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, LoopRole, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::TirValue;

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

    fn make_op_with_attrs(
        opcode: OpCode,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
        attrs: AttrDict,
    ) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn lowers_checked_i64_add_with_overflow_side_channels() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![],
                ops: vec![
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(0)]),
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(1)]),
                    make_op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]),
                ],
                terminator: Terminator::Return {
                    values: vec![ValueId(2)],
                },
            },
        );
        let func = TirFunction {
            name: "checked_add".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::I64,
            blocks,
            entry_block: entry,
            next_value: 3,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func, None);
        let add = &lir.blocks[&entry].ops[2];
        assert_eq!(add.result_values.len(), 3);
        assert_eq!(
            add.tir_op.attrs.get("lir.checked_overflow"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn lower_return_values_follow_lir_return_surface_not_raw_function_return_type() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![],
                ops: vec![make_op(OpCode::ConstNone, vec![], vec![ValueId(0)])],
                terminator: Terminator::Return {
                    values: vec![ValueId(0)],
                },
            },
        );
        let func = TirFunction {
            name: "implicit_raise_helper".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry,
            next_value: 1,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func, None);
        assert_eq!(lir.return_types, vec![TirType::None]);
        match &lir.blocks[&entry].terminator {
            LirTerminator::Return { values } => assert_eq!(values.len(), 1),
            other => panic!("expected return terminator, got {other:?}"),
        }
    }

    #[test]
    fn heap_user_class_allocation_stays_boxed() {
        let entry = BlockId(0);
        let class_ref = ValueId(0);
        let instance = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![TirValue {
                    id: class_ref,
                    ty: TirType::DynBox,
                }],
                ops: vec![make_op_with_attrs(
                    OpCode::ObjectNewBound,
                    vec![class_ref],
                    vec![instance],
                    attrs,
                )],
                terminator: Terminator::Return {
                    values: vec![instance],
                },
            },
        );
        let func = TirFunction {
            name: "alloc_point".into(),
            param_names: vec!["cls".into()],
            param_types: vec![TirType::DynBox],
            return_type: TirType::UserClass("Point".into()),
            blocks,
            entry_block: entry,
            next_value: 2,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func, None);
        let alloc = &lir.blocks[&entry].ops[0];
        assert_eq!(
            alloc.result_values[0].ty,
            TirType::UserClass("Point".into())
        );
        assert_eq!(alloc.result_values[0].repr, LirRepr::DynBox);
    }

    #[test]
    fn stack_user_class_allocation_lowers_to_ref64() {
        let entry = BlockId(0);
        let class_ref = ValueId(0);
        let instance = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
        attrs.insert("value".into(), AttrValue::Int(16));
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![TirValue {
                    id: class_ref,
                    ty: TirType::DynBox,
                }],
                ops: vec![make_op_with_attrs(
                    OpCode::ObjectNewBoundStack,
                    vec![class_ref],
                    vec![instance],
                    attrs,
                )],
                terminator: Terminator::Return {
                    values: vec![instance],
                },
            },
        );
        let func = TirFunction {
            name: "stack_alloc_point".into(),
            param_names: vec!["cls".into()],
            param_types: vec![TirType::DynBox],
            return_type: TirType::UserClass("Point".into()),
            blocks,
            entry_block: entry,
            next_value: 2,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func, None);
        let alloc = &lir.blocks[&entry].ops[0];
        assert_eq!(
            alloc.result_values[0].ty,
            TirType::UserClass("Point".into())
        );
        assert_eq!(alloc.result_values[0].repr, LirRepr::Ref64);
    }

    #[test]
    fn non_executable_loop_end_edges_do_not_lower_to_lir_branches() {
        let entry = BlockId(0);
        let header = BlockId(1);
        let exit = BlockId(2);
        let dead_loop_end = BlockId(3);

        let i_init = ValueId(0);
        let i = ValueId(1);
        let dead_none = ValueId(2);

        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![],
                ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![i_init],
                },
            },
        );
        blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: i,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![],
                },
            },
        );
        blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        blocks.insert(
            dead_loop_end,
            TirBlock {
                id: dead_loop_end,
                args: vec![],
                ops: vec![make_op(OpCode::ConstNone, vec![], vec![dead_none])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![dead_none],
                },
            },
        );

        let func = TirFunction {
            name: "dead_loop_end_lir".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry,
            next_value: 3,
            next_block: 4,
            attrs: AttrDict::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::from([(dead_loop_end, LoopRole::LoopEnd)]),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func, None);
        assert!(crate::tir::verify_lir::verify_lir_function(&lir).is_ok());
        assert!(matches!(
            lir.blocks[&dead_loop_end].terminator,
            LirTerminator::Unreachable
        ));
    }
}
