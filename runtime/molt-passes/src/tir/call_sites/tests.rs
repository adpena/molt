use super::*;
use crate::tir::blocks::Terminator;
use crate::tir::call_facts::{CallFactsTable, CallTargetFact};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::TirModule;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn op(opcode: OpCode, args: &[ValueId], results: &[ValueId]) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: args.to_vec(),
        results: results.to_vec(),
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn function() -> TirFunction {
    let mut func = TirFunction::new(
        "callback".into(),
        vec![TirType::I64, TirType::I64],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func
}

fn append(func: &mut TirFunction, operation: TirOp) -> (BlockId, usize) {
    let block = func.blocks.get_mut(&func.entry_block).unwrap();
    let site = (block.id, block.ops.len());
    block.ops.push(operation);
    site
}

fn integer(func: &mut TirFunction, value: i64) -> ValueId {
    let result = func.fresh_value();
    let mut constant = op(OpCode::ConstInt, &[], &[result]);
    constant.attrs.insert("value".into(), AttrValue::Int(value));
    append(func, constant);
    result
}

#[test]
fn dynamic_protocol_families_are_callbacks_despite_scalar_annotations() {
    for (opcode, arity, results) in [
        (OpCode::Add, 2, 1),
        (OpCode::Sub, 2, 1),
        (OpCode::Mul, 2, 1),
        (OpCode::CheckedAdd, 2, 2),
        (OpCode::CheckedMul, 2, 2),
        (OpCode::InplaceAdd, 2, 1),
        (OpCode::InplaceSub, 2, 1),
        (OpCode::InplaceMul, 2, 1),
        (OpCode::Div, 2, 1),
        (OpCode::FloorDiv, 2, 1),
        (OpCode::Mod, 2, 1),
        (OpCode::Pow, 2, 1),
        (OpCode::Neg, 1, 1),
        (OpCode::Pos, 1, 1),
        (OpCode::Eq, 2, 1),
        (OpCode::Ne, 2, 1),
        (OpCode::Lt, 2, 1),
        (OpCode::Le, 2, 1),
        (OpCode::Gt, 2, 1),
        (OpCode::Ge, 2, 1),
        (OpCode::In, 2, 1),
        (OpCode::NotIn, 2, 1),
        (OpCode::BitAnd, 2, 1),
        (OpCode::BitOr, 2, 1),
        (OpCode::BitXor, 2, 1),
        (OpCode::BitNot, 1, 1),
        (OpCode::Shl, 2, 1),
        (OpCode::Shr, 2, 1),
        (OpCode::And, 2, 1),
        (OpCode::Or, 2, 1),
        (OpCode::Not, 1, 1),
        (OpCode::Bool, 1, 1),
        (OpCode::LoadAttr, 1, 1),
        (OpCode::StoreAttr, 2, 0),
        (OpCode::DelAttr, 1, 0),
        (OpCode::Index, 2, 1),
        (OpCode::StoreIndex, 3, 0),
        (OpCode::DelIndex, 2, 0),
        (OpCode::GetIter, 1, 1),
        (OpCode::IterNext, 1, 1),
        (OpCode::DecRef, 1, 0),
        (OpCode::DeleteVar, 2, 1),
        (OpCode::ClosureStore, 2, 0),
        (OpCode::ModuleCacheGet, 1, 1),
    ] {
        let mut func = function();
        let output: Vec<_> = (0..results).map(|_| func.fresh_value()).collect();
        let args: Vec<_> = (0..arity).map(|i| ValueId(i % 2)).collect();
        let site = append(&mut func, op(opcode, &args, &output));
        let sites = FunctionCallSites::for_function(&func);
        assert_eq!(
            sites.at(site).unwrap().target,
            CallSiteTarget::Opaque,
            "{opcode:?}"
        );
        let local = CallFactsTable::build_local(&func);
        if let Some(result) = output.first() {
            assert_eq!(
                local.get(*result).unwrap().target,
                CallTargetFact::Opaque,
                "{opcode:?}"
            );
        }
        let graph = CallGraph::build(&TirModule {
            name: "m".into(),
            functions: vec![func],
        });
        assert!(!graph.leaf_functions().contains("callback"), "{opcode:?}");
        assert!(graph.has_opaque_call("callback"), "{opcode:?}");
    }
}

#[test]
fn exact_scalar_operations_and_releases_keep_leaf_precision() {
    for opcode in [
        OpCode::Add,
        OpCode::CheckedAdd,
        OpCode::CheckedMul,
        OpCode::FloorDiv,
        OpCode::Neg,
        OpCode::Pos,
        OpCode::Eq,
        OpCode::Bool,
        OpCode::DecRef,
    ] {
        let mut func = function();
        let a = integer(&mut func, 12);
        let b = integer(&mut func, 3);
        let arity = if matches!(
            opcode,
            OpCode::Neg | OpCode::Pos | OpCode::Bool | OpCode::DecRef
        ) {
            1
        } else {
            2
        };
        let count = if opcode == OpCode::DecRef {
            0
        } else if matches!(opcode, OpCode::CheckedAdd | OpCode::CheckedMul) {
            2
        } else {
            1
        };
        let results: Vec<_> = (0..count).map(|_| func.fresh_value()).collect();
        let site = append(&mut func, op(opcode, &[a, b][..arity], &results));
        assert!(
            FunctionCallSites::for_function(&func).at(site).is_none(),
            "{opcode:?}"
        );
        let graph = CallGraph::build(&TirModule {
            name: "m".into(),
            functions: vec![func],
        });
        assert!(graph.leaf_functions().contains("callback"), "{opcode:?}");
    }
}

#[test]
fn fixed_slot_proofs_exclude_guarded_and_releasing_accesses() {
    let mut func = function();
    let object = func.fresh_value();
    let mut alloc = op(OpCode::Alloc, &[], &[object]);
    alloc.attrs.insert("value".into(), AttrValue::Int(16));
    append(&mut func, alloc);
    let neutral = integer(&mut func, 7);
    let mut store = op(OpCode::StoreAttr, &[object, neutral], &[]);
    store
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    store.attrs.insert("value".into(), AttrValue::Int(0));
    let first = append(&mut func, store.clone());
    let result = func.fresh_value();
    let mut load = op(OpCode::LoadAttr, &[object], &[result]);
    load.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    load.attrs.insert("value".into(), AttrValue::Int(0));
    let plain = append(&mut func, load.clone());
    load.results = vec![func.fresh_value()];
    load.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("guarded_load".into()),
    );
    let guarded = append(&mut func, load);
    let after_callback = append(&mut func, store);
    let sites = FunctionCallSites::for_function(&func);
    assert!(sites.at(first).is_none());
    assert!(sites.at(plain).is_none());
    assert!(sites.at(guarded).is_some());
    assert!(sites.at(after_callback).is_some());
}

#[test]
fn polls_and_unknown_preserved_ops_fail_closed_but_inert_primitives_do_not() {
    for (kind, callback) in [
        ("future_runtime_operation", true),
        ("builtin_func", true),
        ("trace_enter_slot", true),
        ("trace_exit", true),
        ("line", false),
        ("print_newline", false),
        ("call_async", false),
    ] {
        let mut func = function();
        let result = func.fresh_value();
        let mut copy = op(OpCode::Copy, &[], &[result]);
        copy.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        copy.attrs
            .insert("s_value".into(), AttrValue::Str("ignored".into()));
        let position = append(&mut func, copy);
        assert_eq!(
            FunctionCallSites::for_function(&func)
                .at(position)
                .is_some(),
            callback,
            "{kind}"
        );
    }
    let mut func = function();
    let ordinary = append(&mut func, op(OpCode::CheckException, &[], &[]));
    let mut poll = op(OpCode::CheckException, &[], &[]);
    poll.mark_async_work_poll();
    let marked = append(&mut func, poll);
    let sites = FunctionCallSites::for_function(&func);
    assert!(sites.at(ordinary).is_none());
    assert!(sites.at(marked).unwrap().effects.may_call_python);
}

#[test]
fn preserved_direct_calls_keep_targets_and_unknown_copy_cannot_borrow_them() {
    for (kind, direct) in [
        ("call", true),
        ("call_internal", true),
        ("call_func", false),
        ("unknown", false),
    ] {
        let mut func = function();
        let result = func.fresh_value();
        let mut copy = op(OpCode::Copy, &[], &[result]);
        copy.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        copy.attrs
            .insert("s_value".into(), AttrValue::Str("callee".into()));
        let position = append(&mut func, copy);
        let sites = FunctionCallSites::for_function(&func);
        assert_eq!(
            matches!(
                sites.at(position).unwrap().target,
                CallSiteTarget::Direct(_)
            ),
            direct,
            "{kind}"
        );
        assert!(
            CallFactsTable::build_local(&func).get(result).is_some(),
            "{kind}"
        );
    }
}

#[test]
fn conditional_terminators_share_the_exact_truthiness_oracle() {
    for known_scalar in [false, true] {
        let mut func = function();
        let cond = if known_scalar {
            integer(&mut func, 1)
        } else {
            ValueId(0)
        };
        let entry = func.entry_block;
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: entry,
            then_args: vec![ValueId(0), ValueId(1)],
            else_block: entry,
            else_args: vec![ValueId(0), ValueId(1)],
        };
        assert_eq!(
            FunctionCallSites::for_function(&func)
                .terminator_at(entry)
                .is_some(),
            !known_scalar
        );
        let graph = CallGraph::build(&TirModule {
            name: "m".into(),
            functions: vec![func],
        });
        assert_eq!(graph.leaf_functions().contains("callback"), known_scalar);
    }
}

#[test]
fn trace_lifecycle_revokes_slot_history_and_leaf_admission() {
    for kind in ["trace_enter_slot", "trace_exit"] {
        let mut func = function();
        let object = func.fresh_value();
        let mut alloc = op(OpCode::Alloc, &[], &[object]);
        alloc.attrs.insert("value".into(), AttrValue::Int(16));
        append(&mut func, alloc);
        let neutral = integer(&mut func, 7);
        let mut store = op(OpCode::StoreAttr, &[object, neutral], &[]);
        store
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        store.attrs.insert("value".into(), AttrValue::Int(0));
        let before = append(&mut func, store.clone());
        let mut trace = op(OpCode::Copy, &[], &[]);
        trace
            .attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        let lifecycle = append(&mut func, trace);
        let after = append(&mut func, store);
        let sites = FunctionCallSites::for_function(&func);
        assert!(sites.at(before).is_none());
        assert_eq!(sites.at(lifecycle).unwrap().target, CallSiteTarget::Opaque);
        assert!(sites.at(after).is_some());
        let graph = CallGraph::build(&TirModule {
            name: "m".into(),
            functions: vec![func],
        });
        assert!(!graph.leaf_functions().contains("callback"));
    }
}

#[test]
fn checked_boxed_callbacks_can_capture_operands() {
    for opcode in [OpCode::CheckedAdd, OpCode::CheckedMul] {
        let mut func = function();
        let list = func.fresh_value();
        append(&mut func, op(OpCode::BuildList, &[], &[list]));
        let result = func.fresh_value();
        let overflow = func.fresh_value();
        append(
            &mut func,
            op(opcode, &[list, ValueId(0)], &[result, overflow]),
        );
        let escapes = crate::tir::passes::escape_analysis::analyze(&func);
        assert_eq!(
            escapes[&list],
            crate::tir::passes::escape_analysis::EscapeState::GlobalEscape
        );
    }
}
