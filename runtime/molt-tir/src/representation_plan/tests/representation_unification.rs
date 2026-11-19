//! Each integer operator keeps a raw inline carrier only within the proven range.
//! The same SSA plan drives the two native instruction selectors and WASM.

use crate::repr::Repr;
use crate::representation_plan::{repr_by_value_for, value_range_for};
use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

#[derive(Clone, Copy)]
struct IntOp {
    /// SimpleIR `OpIR.kind` for the body op `r = i <op> const`.
    simple_kind: molt_ir::OpKind,
    /// Canonical TIR opcode for the same op.
    tir_opcode: OpCode,
    /// Constant right-hand operand for the body op. Chosen so that for a
    /// bounded IV `i in [0, IV_STOP)` the result still fits the signed
    /// inline-int47 window `[-2^46, 2^46 - 1]`.
    rhs_const: i64,
    /// Expected admission shared by the VALUE-KEYED and NAME-KEYED authority:
    /// `repr_by_value_for` on canonical TIR supplies the proof consumed by the
    /// LLVM/WASM backends and the name projection.
    /// `mod`/`floordiv`/`add`/`mul`/`shl` have value-range transfer rules and
    /// are admitted when their bounded result stays in the inline-int47 window.
    expected_raw: bool,
}

/// The bounded induction-variable ceiling. Small enough that `i * RHS`,
/// `i + RHS`, `i << 1`, `i // RHS`, and `i % RHS` all stay inside
/// inline-int47 for every `i in [0, IV_STOP)`.
const IV_STOP: i64 = 1_000;

fn int_ops() -> Vec<IntOp> {
    vec![
        IntOp {
            simple_kind: molt_ir::OpKind::Mod,
            tir_opcode: OpCode::Mod,
            rhs_const: 7,
            expected_raw: true,
        },
        IntOp {
            // `//` has a value-range transfer rule at both authorities, so a
            // bounded `i // 3` result stays on the raw carrier lane.
            simple_kind: molt_ir::OpKind::Floordiv,
            tir_opcode: OpCode::FloorDiv,
            rhs_const: 3,
            expected_raw: true,
        },
        IntOp {
            simple_kind: molt_ir::OpKind::Add,
            tir_opcode: OpCode::Add,
            rhs_const: 7,
            expected_raw: true,
        },
        IntOp {
            simple_kind: molt_ir::OpKind::Mul,
            tir_opcode: OpCode::Mul,
            rhs_const: 7,
            expected_raw: true,
        },
        IntOp {
            // Exact integer provenance, inline result range and a count in
            // [0, 63] admit the same raw carrier in both value and name views.
            simple_kind: molt_ir::OpKind::Lshift,
            tir_opcode: OpCode::Shl,
            rhs_const: 1,
            expected_raw: true,
        },
    ]
}

fn tir_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::default(),
        source_span: None,
    }
}

fn tir_op_nsw(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut o = tir_op(opcode, operands, results);
    o.attrs
        .insert("no_signed_wrap".into(), AttrValue::Bool(true));
    o
}

fn tir_cint(result: ValueId, value: i64) -> TirOp {
    let mut o = tir_op(OpCode::ConstInt, vec![], vec![result]);
    o.attrs.insert("value".into(), AttrValue::Int(value));
    o
}

/// Build `for i in range(0, stop): body = i <op> const` directly in TIR: a
/// header block-arg IV with a `no_signed_wrap` increment (the AddRec shape
/// SCEV recognises and value-range turns into a proven `[0, stop)` range),
/// plus the body op whose result `body` we probe. Returns `(func, body_value)`.
fn bounded_loop_body_op_tir(int_op: IntOp, stop: i64) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "rl".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let startc = func.fresh_value();
    let stopc = func.fresh_value();
    let stepc = func.fresh_value();
    let rhsc = func.fresh_value();
    let iv = func.fresh_value();
    let cond = func.fresh_value();
    let body = func.fresh_value();
    let next = func.fresh_value();

    let header = func.fresh_block();
    let body_block = func.fresh_block();
    let exit = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            tir_cint(startc, 0),
            tir_cint(stopc, stop),
            tir_cint(stepc, 1),
            tir_cint(rhsc, int_op.rhs_const),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![startc],
        };
    }
    // Type every integer value as real lowered TIR would (every int refines to
    // I64), so the representation floor maps them to MaybeBigInt rather than the
    // unknown-type DynBox, and the value-range raise is the only thing that can
    // lift them to RawI64Safe.
    for v in [startc, stopc, stepc, rhsc, iv, body, next] {
        func.value_types.insert(v, TirType::I64);
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: iv,
                ty: TirType::I64,
            }],
            ops: vec![tir_op(OpCode::Lt, vec![iv, stopc], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body_block,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.blocks.insert(
        body_block,
        TirBlock {
            id: body_block,
            args: vec![],
            ops: vec![
                tir_op(int_op.tir_opcode, vec![iv, rhsc], vec![body]),
                tir_op_nsw(OpCode::Add, vec![iv, stepc], vec![next]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);
    (func, body)
}

#[test]
fn value_range_proof_and_carrier_registry_agree_per_int_op() {
    for int_op in int_ops() {
        let (func, body) = bounded_loop_body_op_tir(int_op, IV_STOP);
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));

        let proven_inline = vr.fits_inline_int47(body);
        let registry_raw = repr
            .get(&body)
            .copied()
            .is_some_and(|r| r.is_raw_i64_carrier());

        // The design's literal section-5 assertion: the registry (the single source
        // the instruction selectors consume) AGREES with the
        // value-range proof. Equality in BOTH directions - a proven value is
        // raw, a non-proven value is never raw. This is the producer<->consumer
        // agreement that makes the modulo P0 (store-boxed / read-raw)
        // unexpressible, holding uniformly across the int op family.
        assert_eq!(
            registry_raw, proven_inline,
            "{}: carrier registry (raw={registry_raw}) must agree with the \
             value-range proof (fits_inline_int47={proven_inline}) for the \
             body op result",
            int_op.simple_kind
        );

        // Pin the expected proof direction per op so a silent change in
        // value-range coverage (a new rule that starts proving `//`, or a lost
        // rule that stops proving `mod`) is caught rather than silently
        // flipping the agreement to the other (still self-consistent) branch.
        assert_eq!(
            proven_inline, int_op.expected_raw,
            "{}: value-range proof direction changed (fits_inline_int47={proven_inline}, \
             expected {})",
            int_op.simple_kind, int_op.expected_raw
        );
        if proven_inline {
            // A proven-bounded result must be the inline-safe tier specifically.
            assert_eq!(
                repr.get(&body).copied(),
                Some(Repr::RawI64Safe),
                "{}: a proven-bounded result must be RawI64Safe in the registry",
                int_op.simple_kind
            );
        } else {
            // A non-proven result stays the boxed BigInt-safe default - never a
            // raw carrier (which would be the trusted-unbox corruption class).
            assert!(
                !registry_raw,
                "{}: a non-proven result must NOT be a raw carrier",
                int_op.simple_kind
            );
        }
    }
}

// ======================================================================
// Part C - NEGATIVE CONTROL.
//
// A fragmenting edit is "store a boxed result as a raw carrier" - i.e. the
// authority claims raw for a value the proof does NOT admit inline (or, dually,
// drops a proven value off the raw lane). Both directions must be REJECTED.
// ======================================================================

/// The agreement predicate the gate enforces: a value is admitted to the raw
/// carrier IFF the value-range proof admits it inline-int47. A fragmenting edit
/// breaks exactly this equality.
fn carrier_agrees_with_proof(registry_raw: bool, proven_inline: bool) -> bool {
    registry_raw == proven_inline
}

#[test]
fn overflowing_result_is_refused_by_both_proof_and_registry() {
    // `i * 2^40` with `i in [0, 2^20)` reaches ~2^60, far outside inline-int47.
    // Both the proof and the registry must REFUSE the raw carrier; if a
    // fragmenting store promoted this boxed result to the raw lane (the modulo
    // P0 in reverse - a > 2^46 value trusted-unboxed) the agreement breaks.
    let overflow_op = IntOp {
        simple_kind: molt_ir::OpKind::Mul,
        tir_opcode: OpCode::Mul,
        rhs_const: 1i64 << 40,
        // The whole point of this control: the result is NOT proven inline
        // (it overflows 2^46), so it must be refused the raw carrier at BOTH
        // authorities.
        expected_raw: false,
    };
    let big_stop = 1i64 << 20;
    let (func, body) = bounded_loop_body_op_tir(overflow_op, big_stop);
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));

    let proven_inline = vr.fits_inline_int47(body);
    let registry_raw = repr
        .get(&body)
        .copied()
        .is_some_and(|r| r.is_raw_i64_carrier());

    assert!(
        !proven_inline,
        "an `i * 2^40` result (~2^60) must NOT be proven inline-int47"
    );
    assert!(
        !registry_raw,
        "an overflowing result must NOT be admitted to the raw carrier \
         (trusted-unboxing a > 2^46 value is the modulo P0 corruption class)"
    );
    assert!(
        carrier_agrees_with_proof(registry_raw, proven_inline),
        "proof and registry must agree (both refuse) on the overflowing result"
    );
}

#[test]
fn agreement_predicate_rejects_both_fragmentation_directions() {
    // Sanity floor: the predicate accepts genuine agreement.
    assert!(
        carrier_agrees_with_proof(true, true),
        "proven inline + raw carrier is genuine agreement"
    );
    assert!(
        carrier_agrees_with_proof(false, false),
        "not-proven + not-raw is genuine agreement"
    );

    // THE NEGATIVE CONTROL, direction 1 - the exact modulo P0 fragmentation:
    // a boxed result (NOT proven inline) stored as a raw carrier. The predicate
    // MUST reject it, so a store that took the raw `def_var_named` lane for a
    // possibly-boxed value under a raw-carrier name fails the gate.
    assert!(
        !carrier_agrees_with_proof(/* registry_raw */ true, /* proven_inline */ false),
        "NEGATIVE CONTROL: a not-proven value promoted to the raw carrier \
         (boxed result stored raw - the modulo P0) must be REJECTED"
    );

    // Direction 2 - the dual: a proven value dropped off the raw lane (a
    // perf/correctness fragmentation where the fast lane silently disengages).
    assert!(
        !carrier_agrees_with_proof(/* registry_raw */ false, /* proven_inline */ true),
        "NEGATIVE CONTROL: a proven-inline value demoted off the raw carrier \
         must be REJECTED"
    );
}

/// Run the gate's per-value agreement check over a (possibly mutated) carrier
/// map against the value-range proof: returns `Ok(())` iff every value's raw
/// admission matches its `fits_inline_int47` proof. This is the exact invariant
/// the positive tests assert; here it is reified so the negative control can run
/// it against a deliberately FRAGMENTED map and observe the failure.
fn check_full_agreement(
    func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    repr: &molt_ir::deterministic_hash::HashMap<ValueId, Repr>,
) -> Result<(), ValueId> {
    // Check the agreement for one carrier-bearing value: an integer-family
    // value's raw admission in the registry must match its inline-int47 proof.
    // Bool/float/none carriers are a different lane and are not governed by
    // `fits_inline_int47`, so they are skipped.
    let check_value = |value: ValueId| -> Result<(), ValueId> {
        if !matches!(
            func.value_types.get(&value),
            Some(TirType::I64 | TirType::BigInt)
        ) {
            return Ok(());
        }
        let registry_raw = repr
            .get(&value)
            .copied()
            .is_some_and(|r| r.is_raw_i64_carrier());
        let proven_inline = vr.fits_inline_int47(value);
        if registry_raw != proven_inline {
            return Err(value);
        }
        Ok(())
    };
    for block in func.blocks.values() {
        // Block arguments (phis) are carrier-bearing values too - the loop IV
        // phi is the canonical proven-inline phi the dual fragmentation targets.
        for arg in &block.args {
            check_value(arg.id)?;
        }
        for op in &block.ops {
            for &result in &op.results {
                check_value(result)?;
            }
        }
    }
    Ok(())
}

/// NEGATIVE CONTROL on real authority data: take a genuine bounded-loop carrier
/// map (which agrees with the proof), then apply the modulo-P0 fragmentation -
/// promote a NON-proven (boxed) op result to `RawI64Safe`, i.e. "store a boxed
/// result as a raw carrier" - and confirm the gate's agreement check now FAILS.
/// Dually, demote a proven result and confirm that is caught too. This proves
/// the gate is not vacuous: a fragmenting edit is detected.
#[test]
fn fragmented_carrier_map_fails_the_gate() {
    // A wide `i * 2^40` loop: the `body` result is NOT value-range-proven
    // inline, so in the genuine map it is boxed (not raw) - and the genuine
    // map AGREES with the proof.
    let overflowing_mul = IntOp {
        simple_kind: molt_ir::OpKind::Mul,
        tir_opcode: OpCode::Mul,
        rhs_const: 1i64 << 40,
        expected_raw: false,
    };
    let (func, body) = bounded_loop_body_op_tir(overflowing_mul, 1i64 << 20);
    let vr = value_range_for(&func);
    let genuine = repr_by_value_for(&func, Some(&vr));

    // The genuine, unification-correct map passes the gate.
    assert!(
        check_full_agreement(&func, &vr, &genuine).is_ok(),
        "the genuine carrier map must agree with the value-range proof"
    );
    assert!(
        !vr.fits_inline_int47(body),
        "fixture invariant: the `i * 2^40` body result is not proven inline"
    );

    // FRAGMENTATION (the modulo P0): force the boxed `body` result to claim the
    // raw carrier - exactly what a store taking the raw `def_var_named` lane for
    // a possibly-boxed value under a raw-carrier name does. The gate must catch
    // the now-broken producer<->consumer agreement.
    let mut fragmented_promote = genuine.clone();
    fragmented_promote.insert(body, Repr::RawI64Safe);
    assert_eq!(
        check_full_agreement(&func, &vr, &fragmented_promote),
        Err(body),
        "NEGATIVE CONTROL: promoting a non-proven (boxed) result to RawI64Safe \
         (the boxed-stored-as-raw modulo P0) must be caught by the gate"
    );

    // FRAGMENTATION (dual): demote a genuinely-proven result off the raw lane.
    // The IV phi `i` IS value-range-proven inline; demoting it to MaybeBigInt
    // breaks agreement in the other direction.
    let iv = func
        .blocks
        .values()
        .flat_map(|b| b.args.iter())
        .map(|a| a.id)
        .find(|&v| vr.fits_inline_int47(v))
        .expect("the bounded loop must have at least one proven-inline phi (the IV)");
    let mut fragmented_demote = genuine.clone();
    fragmented_demote.insert(iv, Repr::MaybeBigInt);
    assert_eq!(
        check_full_agreement(&func, &vr, &fragmented_demote),
        Err(iv),
        "NEGATIVE CONTROL (dual): demoting a proven-inline value off the raw \
         carrier must be caught by the gate"
    );
}
