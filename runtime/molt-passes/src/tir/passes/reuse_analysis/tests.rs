use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::alias_analysis::AliasAnalysis;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::compat::{SizeClass, size_class};
use super::{ReuseCandidate, analyze, run};

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

fn analyze_fresh(func: &TirFunction) -> Vec<ReuseCandidate> {
    let mut am = AnalysisManager::new();
    let alias = am.get::<AliasAnalysis>(func).clone();
    analyze(func, &alias)
}

fn run_fresh(func: &mut TirFunction) -> PassStats {
    let mut am = AnalysisManager::new();
    run(func, &mut am)
}

#[test]
fn decref_alloc_same_type_produces_candidate() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_x = func.fresh_value();
    let load_result = func.fresh_value();
    let alloc_y = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_x]));
    entry
        .ops
        .push(make_op(OpCode::LoadAttr, vec![alloc_x], vec![load_result]));
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

    let candidates = analyze_fresh(&func);
    assert_eq!(
        candidates.len(),
        1,
        "should find exactly one reuse candidate"
    );
    assert_eq!(candidates[0].decref_value, alloc_x);
    assert_eq!(candidates[0].alloc_value, alloc_y);
}

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

    let candidates = analyze_fresh(&func);
    assert!(
        candidates.is_empty(),
        "barrier between DecRef and Alloc should prevent reuse"
    );
}

#[test]
fn stack_alloc_not_eligible_for_reuse() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let stack_val = func.fresh_value();
    let alloc_y = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
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

    let candidates = analyze_fresh(&func);
    assert!(
        candidates.is_empty(),
        "StackAlloc values should not be reuse candidates"
    );
}

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

    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 1, "should annotate one reuse pair");

    let entry = &func.blocks[&func.entry_block];
    let decref_op = &entry.ops[1];
    assert_eq!(decref_op.opcode, OpCode::DecRef);
    assert_eq!(
        decref_op.attrs.get("reuse_token_id"),
        Some(&AttrValue::Int(0))
    );
    let alloc_op = &entry.ops[2];
    assert_eq!(alloc_op.opcode, OpCode::Alloc);
    assert_eq!(
        alloc_op.attrs.get("reuse_from_token"),
        Some(&AttrValue::Int(0))
    );
}

#[test]
fn multiple_pairs_in_same_block() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let alloc_a = func.fresh_value();
    let alloc_b = func.fresh_value();
    let alloc_c = func.fresh_value();
    let alloc_d = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
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

    let candidates = analyze_fresh(&func);
    assert_eq!(candidates.len(), 2, "should find two reuse pairs");
    assert_eq!(candidates[0].decref_value, alloc_a);
    assert_eq!(candidates[0].alloc_value, alloc_c);
    assert_eq!(candidates[1].decref_value, alloc_b);
    assert_eq!(candidates[1].alloc_value, alloc_d);
}

#[test]
fn decref_on_parameter_not_eligible() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let param = ValueId(0);
    let alloc_y = func.fresh_value();
    let const_none = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::DecRef, vec![param], vec![]));
    entry
        .ops
        .push(make_op(OpCode::Alloc, vec![], vec![alloc_y]));
    entry
        .ops
        .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
    entry.terminator = Terminator::Return {
        values: vec![const_none],
    };

    let candidates = analyze_fresh(&func);
    assert!(
        candidates.is_empty(),
        "DecRef on function parameter should not produce reuse candidate"
    );
}

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

    let candidates = analyze_fresh(&func);
    assert_eq!(
        candidates.len(),
        1,
        "non-aliasing ops should not block reuse"
    );
    assert_eq!(candidates[0].decref_value, alloc_x);
    assert_eq!(candidates[0].alloc_value, alloc_y);
}

#[test]
fn user_class_size_class_matches_on_id() {
    let point_a = TirType::UserClass("Point".into());
    let point_b = TirType::UserClass("Point".into());
    let line = TirType::UserClass("Line".into());

    assert_eq!(size_class(&point_a), size_class(&point_b));
    assert_ne!(size_class(&point_a), size_class(&line));
    assert!(
        matches!(size_class(&point_a), SizeClass::Typed(_)),
        "UserClass should classify as Typed (static layout), not Dynamic"
    );
}
