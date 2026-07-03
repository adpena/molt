use super::*;

#[test]
fn lower_i64_comparison() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn lt(a: i64, b: i64) -> bool { return a < b }
    let mut func = TirFunction::new(
        "cmp_lt".into(),
        vec![TirType::I64, TirType::I64],
        TirType::Bool,
    );
    let v_result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Lt,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_result],
    };

    // The raw `icmp slt` path needs both operands proven exact-i64 carriers;
    // an unproven `i64` parameter carries boxed (`DynBox`) and dispatches the
    // comparison through the runtime. Prove the two parameters here.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [ValueId(0), ValueId(1)] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("icmp slt"),
        "expected signed less-than comparison in IR: {}",
        ir
    );
}

#[test]
fn lower_box_i64() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn box_it(x: i64) -> DynBox { return box(x) }
    let mut func = TirFunction::new("box_i64".into(), vec![TirType::I64], TirType::DynBox);
    let v_boxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![ValueId(0)],
        results: vec![v_boxed],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_boxed],
    };

    // `box(x)` emits the NaN-boxing arithmetic only when `x` is a RAW i64.
    // An unproven `i64` parameter carries already-boxed (`DynBox`), for which
    // `box` is a no-op; prove the parameter raw so the box path is exercised.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    facts
        .repr_by_value
        .insert(ValueId(0), crate::Repr::RawI64Safe);
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    // Should contain the NaN-boxing OR operations
    assert!(
        ir.contains("or i64"),
        "expected NaN-boxing OR in IR: {}",
        ir
    );
    assert!(
        ir.contains("and i64"),
        "expected NaN-boxing AND mask in IR: {}",
        ir
    );
}

#[test]
fn masked_shift_loop_phi_promoted_to_raw_i64_lane() {
    // #43 end-to-end (the perf payoff the value-range phi narrowing exists
    // for): a `DynBox`-declared loop-header phi that the representation plan
    // proves `RawI64Safe` must be carried as a raw `I64` so the in-loop
    // `<<`/`&` emit raw machine `shl`/`and` instead of the boxed
    // `molt_lshift`/`molt_bit_and` runtime. `type_refine` leaves the masked
    // accumulator `DynBox` (its inline-window fit is a value-range-only fact),
    // so without `effective_block_arg_type`'s DynBox->I64 promotion the phi
    // carries boxed and every iteration round-trips through the runtime — the
    // exact regression this guards.
    //
    // Shape:  s_phi: DynBox = phi[ 1 (preheader), band (back-edge) ]
    //         shl  = s_phi << 1
    //         band = shl & MASK            (MASK = 2**32 - 1)
    //         -> header(band)
    // with the plan proving s_phi / shl / band all RawI64Safe.
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    let mut func = TirFunction::new("masked_shift".into(), vec![], TirType::None);
    let s_start = func.fresh_value(); // ConstInt 1
    let mask_c = func.fresh_value(); // ConstInt (2**32 - 1)
    let one_c = func.fresh_value(); // ConstInt 1 (shift count)
    let s_phi = func.fresh_value(); // header phi (DynBox-declared)
    let shl = func.fresh_value(); // s_phi << 1
    let band = func.fresh_value(); // shl & MASK

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    let mk_int = |result: ValueId, v: i64| TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(v));
            m
        },
        source_span: None,
    };
    let mk_bin = |opcode: OpCode, a: ValueId, b: ValueId, r: ValueId| TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![a, b],
        results: vec![r],
        attrs: AttrDict::new(),
        source_span: None,
    };
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            mk_int(s_start, 1),
            mk_int(mask_c, (1i64 << 32) - 1),
            mk_int(one_c, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![s_start],
        };
    }
    // The phi is DECLARED DynBox (as type_refine leaves the masked accumulator).
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: s_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: body,
                args: vec![],
            },
        },
    );
    func.loop_roles
        .insert(header, crate::tir::blocks::LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                mk_bin(OpCode::Shl, s_phi, one_c, shl),
                mk_bin(OpCode::BitAnd, shl, mask_c, band),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![band],
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
    func.loop_roles
        .insert(exit, crate::tir::blocks::LoopRole::LoopEnd);

    // The plan proves the masked accumulator chain RawI64Safe (what the
    // value-range phi narrowing yields end to end). The ConstInts are I64 by
    // their own lowering; the proof here is for the phi + the two op results.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [s_phi, shl, band] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("shl i64"),
        "masked accumulator shift must lower to a RAW machine `shl i64`, not \
             the boxed runtime. IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_lshift"),
        "a RawI64Safe-proven masked shift must NOT call the boxed `molt_lshift`. \
             IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_bit_and"),
        "a RawI64Safe-proven masked `& MASK` must NOT call the boxed \
             `molt_bit_and`. IR:\n{ir}"
    );
    // The header phi must be a raw `i64` phi (promoted from its DynBox
    // declaration) so the back-edge carries the raw masked value.
    assert!(
        ir.contains("phi i64"),
        "the RawI64Safe masked accumulator phi must be a raw `i64` phi. IR:\n{ir}"
    );
}

// ── RPO algorithm tests ──
//
// The RPO algorithm is exercised end-to-end by the integration tests in
// `runtime/molt-backend/tests/llvm_rpo.rs`, which call into
// [`super::compute_function_rpo`] directly with synthetic CFGs covering
// diamonds, loops, switches, deep chains, self-loops, and unreachable
// blocks. Those tests live in a separate test binary and so are not
// blocked by drift in the wider lib test suite.
