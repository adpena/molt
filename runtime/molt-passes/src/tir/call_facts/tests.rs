use super::*;
use crate::repr::Repr;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirModule;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;

/// A function `name` whose entry block makes a `Call` to each callee (with a
/// captured result `ValueId`), plus `extra_ops` filler `ConstNone` ops, then
/// returns. Returns the function and the result `ValueId` of the FIRST call.
fn func_calling(
    name: &str,
    ret: TirType,
    callees: &[&str],
    extra_ops: usize,
) -> (TirFunction, Option<ValueId>) {
    let mut func = TirFunction::new(name.into(), vec![], ret);
    let entry = func.entry_block;
    // Allocate result ids for each call + filler up front (mutable borrow of
    // `func` must not overlap the block borrow).
    let call_results: Vec<ValueId> = (0..callees.len()).map(|_| func.fresh_value()).collect();
    let filler: Vec<ValueId> = (0..extra_ops).map(|_| func.fresh_value()).collect();
    let first_result = call_results.first().copied();
    let block = func.blocks.get_mut(&entry).unwrap();
    for (callee, &res) in callees.iter().zip(&call_results) {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str((*callee).to_string()));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![res],
            attrs,
            source_span: None,
        });
    }
    for v in filler {
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v],
            attrs: AttrDict::new(),
            source_span: None,
        });
    }
    block.terminator = Terminator::Return { values: vec![] };
    (func, first_result)
}

/// A trivial inlinable leaf: a single `ConstNone` op + `Return`. No calls, no
/// handlers, small.
fn leaf_callee(name: &str, ret: TirType) -> TirFunction {
    let mut f = TirFunction::new(name.into(), vec![], ret);
    let entry = f.entry_block;
    let v = f.fresh_value();
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![v],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    f
}

/// A callee with a real exception handler region (`TryStart`/`TryEnd`).
fn callee_with_handlers(name: &str) -> TirFunction {
    let mut f = TirFunction::new(name.into(), vec![], TirType::None);
    let entry = f.entry_block;
    let block = f.blocks.get_mut(&entry).unwrap();
    for oc in [OpCode::TryStart, OpCode::TryEnd] {
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: oc,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
    }
    block.terminator = Terminator::Return { values: vec![] };
    f
}

fn module(funcs: Vec<TirFunction>) -> TirModule {
    TirModule {
        name: "m".into(),
        functions: funcs,
    }
}

/// Build the precise module table for the function named `caller`.
fn module_table_for(m: &TirModule, caller: &str) -> CallFactsTable {
    let cg = CallGraph::build(m);
    let summaries = ModuleSummaries::compute(m, &cg);
    let tti = TargetInfo::native_release_fast();
    let mut tables = CallFactsTable::build_module(m, &cg, &summaries, &tti);
    tables.remove(caller).expect("caller table present")
}

// -- target classification (the #71 typed fact) ---------------------------

#[test]
fn static_direct_target_for_defined_callee() {
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller, leaf_callee("b", TirType::None)]);
    let table = module_table_for(&m, "a");
    let facts = table.get(res).expect("call site recorded");
    assert_eq!(
        facts.target,
        CallTargetFact::StaticDirect { callee: "b".into() }
    );
    assert!(facts.target.is_static_direct());
    assert_eq!(facts.target.static_callee(), Some("b"));
}

#[test]
fn opaque_target_for_extern_callee() {
    // `b` is NOT defined in the module → opaque (extern / cross-batch).
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller]);
    let table = module_table_for(&m, "a");
    let facts = table.get(res).unwrap();
    assert_eq!(facts.target, CallTargetFact::Opaque);
    assert_eq!(facts.target.static_callee(), None);
}

// -- leaf ----------------------------------------------------------------

#[test]
fn leaf_callee_proven_leaf() {
    // `a` calls `b`; `b` is a leaf (no calls) → leaf = Proven.
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller, leaf_callee("b", TirType::None)]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().leaf, FactValue::Proven);
}

#[test]
fn non_leaf_callee_is_false_leaf() {
    // `a` calls `b`; `b` calls `c` → b is not a leaf → leaf = False (a
    // *decided* negative, not Unknown).
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let (b, _) = func_calling("b", TirType::None, &["c"], 0);
    let m = module(vec![caller, b, leaf_callee("c", TirType::None)]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().leaf, FactValue::False);
}

#[test]
fn opaque_target_leaf_is_unknown() {
    // Extern callee → leaf cannot be decided → Unknown (fail-closed), NOT
    // False.
    let (caller, res) = func_calling("a", TirType::None, &["ext"], 0);
    let res = res.unwrap();
    let m = module(vec![caller]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().leaf, FactValue::Unknown);
}

// -- inlinable (single source of truth vs the inliner) -------------------

#[test]
fn inlinable_leaf_is_eligible_and_matches_is_inlineable() {
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let b = leaf_callee("b", TirType::None);
    let m = module(vec![caller, b]);
    let cg = CallGraph::build(&m);
    let summaries = ModuleSummaries::compute(&m, &cg);
    let tti = TargetInfo::native_release_fast();
    let tables = CallFactsTable::build_module(&m, &cg, &summaries, &tti);
    let facts = tables["a"].get(res).unwrap();
    assert_eq!(facts.inlinable, InlineEligibility::Eligible);
    // EQUIVALENCE: the side-table eligibility bool == is_inlineable's bool.
    let b_body = m.functions.iter().find(|f| f.name == "b").unwrap();
    assert_eq!(
        facts.inlinable.is_eligible(),
        is_inlineable(b_body, &cg, &summaries, &tti)
    );
}

#[test]
fn inlinable_why_not_has_handlers() {
    // `a` calls `b`; `b` has a try/except handler region → WhyNot(HasHandlers).
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller, callee_with_handlers("b")]);
    let table = module_table_for(&m, "a");
    let facts = table.get(res).unwrap();
    assert_eq!(
        facts.inlinable,
        InlineEligibility::WhyNot(InlineWhyNot::HasHandlers)
    );
    assert_eq!(facts.inlinable.why_not(), Some(InlineWhyNot::HasHandlers));
    // A handler-bearing callee is NOT no-throw via the callee-handler rule.
    assert_eq!(facts.no_throw, FactValue::Unknown);
}

#[test]
fn inlinable_why_not_recursive() {
    // Direct self-recursion: `a` calls `a`. The recursive set contains `a`,
    // so a call to it is WhyNot(Recursive).
    let (caller, res) = func_calling("a", TirType::None, &["a"], 0);
    let res = res.unwrap();
    let m = module(vec![caller]);
    let table = module_table_for(&m, "a");
    let facts = table.get(res).unwrap();
    // Self-call target IS static-direct (a is defined) and resolves to a's
    // own body, which is in the recursive set.
    assert_eq!(
        facts.inlinable,
        InlineEligibility::WhyNot(InlineWhyNot::Recursive)
    );
}

// -- no_throw ------------------------------------------------------------

#[test]
fn no_throw_proven_for_handlerless_callee() {
    // `b` is a plain leaf with no handlers → calling it is no_throw = Proven.
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller, leaf_callee("b", TirType::None)]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().no_throw, FactValue::Proven);
}

#[test]
fn no_throw_unknown_for_opaque_target() {
    let (caller, res) = func_calling("a", TirType::None, &["ext"], 0);
    let res = res.unwrap();
    let m = module(vec![caller]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().no_throw, FactValue::Unknown);
}

// -- typed_return --------------------------------------------------------

#[test]
fn typed_return_unknown_for_dynbox_result() {
    // The call result's TirType defaults to DynBox (TirFunction::new doesn't
    // type fresh values) → typed_return = None.
    let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    let m = module(vec![caller, leaf_callee("b", TirType::None)]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.get(res).unwrap().typed_return, None);
}

#[test]
fn typed_return_some_for_typed_result() {
    // Tag the call result with a concrete I64 type → typed_return = Some.
    let (mut caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    caller.value_types.insert(res, TirType::I64);
    let m = module(vec![caller, leaf_callee("b", TirType::None)]);
    let table = module_table_for(&m, "a");
    // I64 floors to MaybeBigInt in the Phase-0 lattice.
    assert_eq!(
        table.get(res).unwrap().typed_return,
        Some(Repr::MaybeBigInt)
    );
}

// -- intraprocedural floor (Analysis::compute) is fail-closed -------------

#[test]
fn local_floor_is_fail_closed() {
    // The local floor sees no module: target = Opaque, leaf = Unknown,
    // inlinable = Unknown, no_throw = Unknown (a plain `Call` opcode throws
    // and there is no resolved body) — but typed_return is still local.
    let (mut caller, res) = func_calling("a", TirType::None, &["b"], 0);
    let res = res.unwrap();
    caller.value_types.insert(res, TirType::Str);
    let table = CallFactsTable::build_local(&caller);
    let facts = table.get(res).unwrap();
    assert_eq!(facts.target, CallTargetFact::Opaque);
    assert_eq!(facts.leaf, FactValue::Unknown);
    assert_eq!(facts.inlinable, InlineEligibility::Unknown);
    assert_eq!(facts.no_throw, FactValue::Unknown);
    // typed_return is purely local → still resolved (Str → DynBox carrier).
    assert_eq!(facts.typed_return, Some(Repr::DynBox));
}

#[test]
fn local_floor_never_out_claims_module_table() {
    // MONOTONICITY: for every recorded call site, the local floor's facts are
    // never *stronger* (more Proven / more StaticDirect) than the precise
    // module table's. This is the soundness contract: a cache miss can only
    // miss an opt, never miscompile.
    let (caller, _) = func_calling("a", TirType::None, &["b", "c"], 1);
    let (b, _) = func_calling("b", TirType::None, &["c"], 0); // non-leaf
    let m = module(vec![caller, b, leaf_callee("c", TirType::None)]);
    let cg = CallGraph::build(&m);
    let summaries = ModuleSummaries::compute(&m, &cg);
    let tti = TargetInfo::native_release_fast();
    let module_tables = CallFactsTable::build_module(&m, &cg, &summaries, &tti);
    let a_body = m.functions.iter().find(|f| f.name == "a").unwrap();
    let local = CallFactsTable::build_local(a_body);
    for (res, mfacts) in module_tables["a"].iter() {
        let lfacts = local.get(res).expect("same call sites keyed");
        // The floor's target is always Opaque (weakest).
        assert_eq!(lfacts.target, CallTargetFact::Opaque);
        // The floor never claims Proven where the module table is weaker.
        if lfacts.leaf.is_proven() {
            assert!(mfacts.leaf.is_proven(), "floor out-claimed leaf");
        }
        if lfacts.no_throw.is_proven() {
            assert!(mfacts.no_throw.is_proven(), "floor out-claimed no_throw");
        }
        // The floor never claims Eligible where the module table did not.
        if lfacts.inlinable.is_eligible() {
            assert!(
                mfacts.inlinable.is_eligible(),
                "floor out-claimed inlinable"
            );
        }
    }
}

// -- table mechanics -----------------------------------------------------

#[test]
fn table_records_one_fact_per_call_site() {
    let (caller, _) = func_calling("a", TirType::None, &["b", "c"], 0);
    let m = module(vec![
        caller,
        leaf_callee("b", TirType::None),
        leaf_callee("c", TirType::None),
    ]);
    let table = module_table_for(&m, "a");
    assert_eq!(table.len(), 2, "two call sites → two records");
    assert!(!table.is_empty());
}

#[test]
fn fact_value_from_decided_is_proven_or_false_not_unknown() {
    assert_eq!(FactValue::from_decided(true), FactValue::Proven);
    assert_eq!(FactValue::from_decided(false), FactValue::False);
    assert!(FactValue::Proven.is_proven());
    assert!(!FactValue::Unknown.is_proven());
    assert!(!FactValue::False.is_proven());
}

/// `CallGraph::is_defined` must exist for the typed-target resolution (the
/// classifier reads it instead of the private `classify_call_op`'s `defined`
/// set). This pins that public accessor.
#[test]
fn call_graph_is_defined_accessor() {
    let m = module(vec![leaf_callee("b", TirType::None)]);
    let cg = CallGraph::build(&m);
    assert!(cg.is_defined("b"));
    assert!(!cg.is_defined("nope"));
}
