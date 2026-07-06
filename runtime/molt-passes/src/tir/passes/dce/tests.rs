use super::classify::is_potentially_throwing;
use super::run;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::effect_proof::EffectProof;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

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

// -----------------------------------------------------------------------
// Test 1: unused constant is removed
// -----------------------------------------------------------------------
#[test]
fn unused_constant_removed() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let v0 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v0]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 2: unused arithmetic op is removed
// -----------------------------------------------------------------------
#[test]
fn unused_arithmetic_removed() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::None);
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let sum = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Add, vec![p0, p1], vec![sum]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 3: used value is kept
// -----------------------------------------------------------------------
#[test]
fn used_value_kept() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
    let v0 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v0]));
    entry.terminator = Terminator::Return { values: vec![v0] };

    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
}

// -----------------------------------------------------------------------
// Test 4: unused Call result is kept when the op must run
// -----------------------------------------------------------------------
#[test]
fn unused_call_result_with_runtime_effect_is_kept() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let callee = func.fresh_value();
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    // Pretend callee is a "known" value: const for the callee pointer.
    entry
        .ops
        .push(make_op(OpCode::ConstInt, vec![], vec![callee]));
    entry
        .ops
        .push(make_op(OpCode::Call, vec![callee], vec![result]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);
    // The Call itself must never be removed.
    let ops = &func.blocks[&func.entry_block].ops;
    assert!(ops.iter().any(|o| o.opcode == OpCode::Call));
    // The ConstInt feeding the Call is used by it, so it stays too.
    let _ = stats;
}

#[test]
fn index_kept_when_result_dead() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str],
        TirType::None,
    );
    let container = ValueId(0);
    let key = ValueId(1);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Index, vec![container, key], vec![result]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::Index));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Index),
        "dead index must be preserved because missing keys and bad indices raise"
    );
}

#[test]
fn ord_at_kept_when_result_dead() {
    let mut func = TirFunction::new("f".into(), vec![TirType::Str, TirType::I64], TirType::None);
    let container = ValueId(0);
    let key = ValueId(1);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::OrdAt, vec![container, key], vec![result]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::OrdAt));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::OrdAt),
        "dead ord_at must be preserved because indexing and ord validation raise"
    );
}

#[test]
fn module_get_attr_kept_when_result_dead() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str],
        TirType::None,
    );
    let module = ValueId(0);
    let attr_name = ValueId(1);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ModuleGetAttr,
        vec![module, attr_name],
        vec![result],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::ModuleGetAttr));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ModuleGetAttr),
        "dead module_get_attr must be preserved because missing attributes raise"
    );
}

#[test]
fn module_cache_get_kept_when_result_dead() {
    let mut func = TirFunction::new("f".into(), vec![TirType::Str], TirType::None);
    let module_name = ValueId(0);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ModuleCacheGet,
        vec![module_name],
        vec![result],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::ModuleCacheGet));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ModuleCacheGet),
        "dead module_cache_get must be preserved because invalid names raise"
    );
}

#[test]
fn static_module_class_binding_effect_proof_allows_dead_lookup_removal() {
    let mut func = TirFunction::new("f".into(), vec![TirType::Str, TirType::Str], TirType::None);
    let module_name = ValueId(0);
    let attr_name = ValueId(1);
    let module = func.fresh_value();
    let class_ref = func.fresh_value();

    let mut cache_get = make_op(OpCode::ModuleCacheGet, vec![module_name], vec![module]);
    cache_get.attrs.insert(
        "effect_proof".into(),
        AttrValue::Str(EffectProof::StaticModuleClassBinding.name().into()),
    );
    let mut attr_get = make_op(
        OpCode::ModuleGetAttr,
        vec![module, attr_name],
        vec![class_ref],
    );
    attr_get.attrs.insert(
        "effect_proof".into(),
        AttrValue::Str(EffectProof::StaticModuleClassBinding.name().into()),
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(cache_get);
    entry.ops.push(attr_get);
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 2);
    assert!(
        func.blocks[&func.entry_block].ops.is_empty(),
        "effect-proven static module/class lookup chain should be removable when dead"
    );
}

#[test]
fn module_get_global_kept_when_result_dead() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str],
        TirType::None,
    );
    let module = ValueId(0);
    let name = ValueId(1);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ModuleGetGlobal,
        vec![module, name],
        vec![result],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::ModuleGetGlobal));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ModuleGetGlobal),
        "dead module_get_global must be preserved because missing globals raise"
    );
}

#[test]
fn module_get_name_kept_when_result_dead() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str],
        TirType::None,
    );
    let module = ValueId(0);
    let name = ValueId(1);
    let result = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(
        OpCode::ModuleGetName,
        vec![module, name],
        vec![result],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(is_potentially_throwing(OpCode::ModuleGetName));
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ModuleGetName),
        "dead module_get_name must be preserved because it reads module attributes"
    );
}

fn assert_module_mutation_kept_when_result_dead(opcode: OpCode, operands: Vec<ValueId>) {
    let mut func = TirFunction::new(
        "f".into(),
        operands.iter().map(|_| TirType::DynBox).collect(),
        TirType::None,
    );

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(opcode, operands, vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert!(
        is_potentially_throwing(opcode),
        "{opcode:?} must be modeled as potentially throwing"
    );
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == opcode),
        "{opcode:?} must be preserved because it mutates module runtime state"
    );
}

#[test]
fn module_cache_set_kept_when_result_dead() {
    assert_module_mutation_kept_when_result_dead(
        OpCode::ModuleCacheSet,
        vec![ValueId(0), ValueId(1)],
    );
}

#[test]
fn module_cache_del_kept_when_result_dead() {
    assert_module_mutation_kept_when_result_dead(OpCode::ModuleCacheDel, vec![ValueId(0)]);
}

#[test]
fn module_set_attr_kept_when_result_dead() {
    assert_module_mutation_kept_when_result_dead(
        OpCode::ModuleSetAttr,
        vec![ValueId(0), ValueId(1), ValueId(2)],
    );
}

#[test]
fn module_del_global_kept_when_result_dead() {
    assert_module_mutation_kept_when_result_dead(
        OpCode::ModuleDelGlobal,
        vec![ValueId(0), ValueId(1)],
    );
}

#[test]
fn module_del_global_if_present_kept_when_result_dead() {
    assert_module_mutation_kept_when_result_dead(
        OpCode::ModuleDelGlobalIfPresent,
        vec![ValueId(0), ValueId(1)],
    );
}

// -----------------------------------------------------------------------
// Test 5: cascade where A feeds B, B feeds C, and C is unused
// -----------------------------------------------------------------------
#[test]
fn cascade_removal() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let a = func.fresh_value();
    let b = func.fresh_value();
    let c = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![a]));
    entry.ops.push(make_op(OpCode::Neg, vec![a], vec![b]));
    entry.ops.push(make_op(OpCode::Neg, vec![b], vec![c]));
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 3);
    assert!(func.blocks[&func.entry_block].ops.is_empty());
}

// -----------------------------------------------------------------------
// Test 6: block argument is never removed
// -----------------------------------------------------------------------
#[test]
fn block_arg_not_removed() {
    // Build: entry -> loop_body(v_arg) -> loop_body   (trivial loop)
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let loop_id = func.fresh_block();
    let v_arg_id = func.fresh_value();

    // Entry branches unconditionally to loop with an arg.
    {
        // Produce the initial arg value (before borrowing blocks mutably).
        let init = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::ConstInt, vec![], vec![init]));
        entry.terminator = Terminator::Branch {
            target: loop_id,
            args: vec![init],
        };
    }

    // Loop block has one block argument; it loops back to itself passing
    // the same arg, so the arg is "live" via the branch.
    let loop_block = TirBlock {
        id: loop_id,
        args: vec![TirValue {
            id: v_arg_id,
            ty: TirType::I64,
        }],
        ops: vec![],
        terminator: Terminator::Branch {
            target: loop_id,
            args: vec![v_arg_id],
        },
    };
    func.blocks.insert(loop_id, loop_block);

    run(&mut func);

    // Block arguments on loop_id must not have been touched.
    assert_eq!(func.blocks[&loop_id].args.len(), 1);
}

// -----------------------------------------------------------------------
// Test 7: empty function: no changes, no panic
// -----------------------------------------------------------------------
#[test]
fn empty_function_no_change() {
    let mut func = TirFunction::new("empty".into(), vec![], TirType::None);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
}
