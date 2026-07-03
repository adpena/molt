use super::super::super::blocks::{Terminator, TirBlock};
use super::super::super::function::{TirFunction, TirModule};
use super::super::super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::super::super::types::TirType;
use super::super::super::values::ValueId;
use super::*;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn const_str(func: &mut TirFunction, s: &str) -> (TirOp, ValueId) {
    let r = func.fresh_value();
    let mut o = op(OpCode::ConstStr, vec![], vec![r]);
    o.attrs.insert("s_value".into(), AttrValue::Str(s.into()));
    (o, r)
}

fn const_int(func: &mut TirFunction, v: i64) -> (TirOp, ValueId) {
    let r = func.fresh_value();
    let mut o = op(OpCode::ConstInt, vec![], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(v));
    func.value_types.insert(r, TirType::I64);
    (o, r)
}

/// The bench_sum chunk shape: preheader sets total/i/N as module attrs,
/// a jump-shaped while loop reads/writes them per iteration with a
/// CheckException (handler label 7 â†’ block 4), exit reads total.
fn module_loop_func() -> TirFunction {
    let mut f = TirFunction::new("chunk".into(), vec![TirType::DynBox], TirType::DynBox);
    let m = ValueId(0);
    let header = f.fresh_block();
    let body = f.fresh_block();
    let exit = f.fresh_block();
    let handler = f.fresh_block();

    // Preheader (entry): total = 0; i = 0; N = 100.
    let (ct0, ct0v) = const_str(&mut f, "total");
    let (zero_op, zero) = const_int(&mut f, 0);
    let (ci0, ci0v) = const_str(&mut f, "i");
    let (cn0, cn0v) = const_str(&mut f, "N");
    let (n_op, nval) = const_int(&mut f, 100);
    {
        let e = f.entry_block;
        let entry = f.blocks.get_mut(&e).unwrap();
        entry.ops = vec![
            ct0,
            zero_op,
            op(OpCode::ModuleSetAttr, vec![m, ct0v, zero], vec![]),
            ci0,
            op(OpCode::ModuleSetAttr, vec![m, ci0v, zero], vec![]),
            cn0,
            n_op,
            op(OpCode::ModuleSetAttr, vec![m, cn0v, nval], vec![]),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }

    // Header: vi = get i; vn = get N; cond = Lt(vi, vn); CondBranch.
    let (ci1, ci1v) = const_str(&mut f, "i");
    let vi = f.fresh_value();
    let (cn1, cn1v) = const_str(&mut f, "N");
    let vn = f.fresh_value();
    let cond = f.fresh_value();
    f.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![
                ci1,
                op(OpCode::ModuleGetAttr, vec![m, ci1v], vec![vi]),
                cn1,
                op(OpCode::ModuleGetAttr, vec![m, cn1v], vec![vn]),
                op(OpCode::Lt, vec![vi, vn], vec![cond]),
            ],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    // Body: vt = get total; sum = Add(vt, vi); CheckException(label 7);
    // set total = sum; ni = Add(vi, 1); set i = ni; Branch header.
    let (ct1, ct1v) = const_str(&mut f, "total");
    let vt = f.fresh_value();
    let sum = f.fresh_value();
    let (ct2, ct2v) = const_str(&mut f, "total");
    let (one_op, one) = const_int(&mut f, 1);
    let ni = f.fresh_value();
    let (ci2, ci2v) = const_str(&mut f, "i");
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(7));
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                ct1,
                op(OpCode::ModuleGetAttr, vec![m, ct1v], vec![vt]),
                op(OpCode::Add, vec![vt, vi], vec![sum]),
                check,
                ct2,
                op(OpCode::ModuleSetAttr, vec![m, ct2v, sum], vec![]),
                one_op,
                op(OpCode::Add, vec![vi, one], vec![ni]),
                ci2,
                op(OpCode::ModuleSetAttr, vec![m, ci2v, ni], vec![]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    // Exit: r = get total; return r.
    let (ct3, ct3v) = const_str(&mut f, "total");
    let r = f.fresh_value();
    f.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![ct3, op(OpCode::ModuleGetAttr, vec![m, ct3v], vec![r])],
            terminator: Terminator::Return { values: vec![r] },
        },
    );

    // Handler (label 7): bare return (the function exception exit).
    f.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f.label_id_map.insert(handler.0, 7);
    f
}

fn count_module_ops_in(func: &TirFunction, blocks: &[BlockId]) -> usize {
    blocks
        .iter()
        .map(|b| {
            func.blocks[b]
                .ops
                .iter()
                .filter(|o| matches!(o.opcode, OpCode::ModuleGetAttr | OpCode::ModuleSetAttr))
                .count()
        })
        .sum()
}

#[test]
fn promotes_bench_sum_shaped_loop() {
    let f = module_loop_func();
    let header = BlockId(1);
    let body = BlockId(2);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert_eq!(changed, vec!["chunk".to_string()], "function promoted");
    assert_eq!(stats.slots_promoted, 3, "total, i, N promoted");
    assert_eq!(stats.ops_eliminated, 5, "3 gets + 2 sets eliminated");
    let f = &module.functions[0];
    assert_eq!(
        count_module_ops_in(f, &[header, body]),
        0,
        "no module-attr traffic left inside the loop"
    );
    assert_eq!(f.blocks[&header].args.len(), 3, "carried phis added");
    // The merged function is structurally valid SSA.
    crate::tir::verify::verify_function(f)
        .unwrap_or_else(|e| panic!("promoted fn invalid: {e:?}"));
    // A compensation block exists: some block (â‰  original handler) carries
    // ModuleSetAttr ops AND branches to the handler block (BlockId(4)).
    let handler = BlockId(4);
    let comp_exists = f.blocks.values().any(|b| {
        b.id != handler
            && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
            && matches!(
                &b.terminator,
                Terminator::Branch { target, .. } if *target == handler
            )
    });
    assert!(comp_exists, "CheckException compensation block present");
    // The exit path stores the dirty slots back (an edge block with sets
    // branching to the original exit block).
    let exit = BlockId(3);
    let exit_store_exists = f.blocks.values().any(|b| {
        b.id != exit
            && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
            && matches!(
                &b.terminator,
                Terminator::Branch { target, .. } if *target == exit
            )
    });
    assert!(exit_store_exists, "exit-edge store-back block present");
}

#[test]
fn threading_import_disables_promotion_module_wide() {
    let f = module_loop_func();
    // A second function importing `threading` â€” a concurrent observer of
    // module globals may then exist.
    let mut g = TirFunction::new("spawner".into(), vec![], TirType::None);
    let imp_res = g.fresh_value();
    let mut imp = op(OpCode::Import, vec![], vec![imp_res]);
    imp.attrs
        .insert("s_value".into(), AttrValue::Str("threading".into()));
    {
        let e = g.entry_block;
        let entry = g.blocks.get_mut(&e).unwrap();
        entry.ops = vec![imp];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f, g],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert!(
        changed.is_empty(),
        "threading import => module-wide refusal"
    );
    assert_eq!(stats.slots_promoted, 0);
}

#[test]
fn thread_intrinsic_name_string_alone_does_not_refuse() {
    // The always-linked stdlib wrapper bodies carry `molt_thread_*` NAME
    // STRINGS (annotations, require_intrinsic args). A mere string must NOT
    // refuse promotion â€” only an Import of threading/_thread or a direct
    // molt_thread_* CALL does. (The over-broad string heuristic refused
    // every program: the needs_inlining trap, round two.)
    let f = module_loop_func();
    let mut g = TirFunction::new("wrapper".into(), vec![], TirType::None);
    let (marker, _) = const_str(&mut g, "molt_thread_spawn");
    {
        let e = g.entry_block;
        let entry = g.blocks.get_mut(&e).unwrap();
        entry.ops = vec![marker];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f, g],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert_eq!(changed, vec!["chunk".to_string()], "string alone is benign");
    assert_eq!(stats.slots_promoted, 3);
}

#[test]
fn call_in_loop_refuses_promotion() {
    let mut f = module_loop_func();
    // Insert an opaque call into the loop body â€” GenericHeap aliases
    // ModuleDict, so the loop must be refused.
    let body = BlockId(2);
    let mut call = op(OpCode::Call, vec![], vec![]);
    call.attrs
        .insert("s_value".into(), AttrValue::Str("opaque".into()));
    f.blocks.get_mut(&body).unwrap().ops.insert(0, call);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert!(changed.is_empty(), "opaque call in loop => refusal");
    assert_eq!(stats.slots_promoted, 0);
}

#[test]
fn typed_field_store_in_loop_does_not_refuse_promotion() {
    // A `guarded_field_set` (TypedField region) writes a class instance's
    // own fixed-layout slot â€” it can NEVER mutate a module-dict slot, so it
    // must NOT disqualify promotion of the module-dict slots in the loop.
    // (Pre-S5-1.5 this op was GenericHeap and aliased ModuleDict, wrongly
    // refusing the loop.) Object identity for the field op is irrelevant; we
    // use a fresh value as the instance.
    let mut f = module_loop_func();
    let body = BlockId(2);
    let inst = f.fresh_value();
    let val = f.fresh_value();
    // guarded_field_set: operands [obj, class_bits, version, val]; the alias
    // oracle only reads operand[0]=obj, offset (`value`), class (`_class`).
    let cbits = f.fresh_value();
    let ver = f.fresh_value();
    let mut fset = op(OpCode::StoreAttr, vec![inst, cbits, ver, val], vec![]);
    fset.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("guarded_field_set".into()),
    );
    fset.attrs.insert("value".into(), AttrValue::Int(0));
    fset.attrs
        .insert("_class".into(), AttrValue::Str("Counter".into()));
    f.blocks.get_mut(&body).unwrap().ops.insert(0, fset);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert_eq!(
        changed,
        vec!["chunk".to_string()],
        "a TypedField store does not alias ModuleDict; promotion proceeds"
    );
    assert_eq!(stats.slots_promoted, 3);
}
