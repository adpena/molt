use super::call_sites::collect_call_sites;
use super::clone_body::{
    clone_attrs_without_simple_names, clone_function_body_with_fresh_ids, exception_label_of,
    function_label_ids,
};
use super::eligibility::is_closure;
use super::splice::splice_call_site;
use super::*;

use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{
    AttrDict, AttrValue, Dialect, OpCode, TirOp, dead_placeholder_const_for_type,
};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// A callee `fn f(a, b) -> a + b` (single block, two params, one add,
/// returns the sum).
fn add_callee() -> TirFunction {
    let mut f = TirFunction::new(
        "addfn".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let sum = f.fresh_value();
    let entry = f.entry_block;
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![p0, p1],
        results: vec![sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![sum] };
    f.value_types.insert(sum, TirType::I64);
    f
}

/// A CLOSURE callee shaped like the frontend's lowering of
/// `def add(x): return base + x` capturing `base`: `param_names =
/// ["__molt_closure__", "x"]`, body unpacks the captured env
/// (`index [__molt_closure__, 0] -> cell; index [cell, 0] -> base`) and
/// returns `base + x`. This is the exact shape that miscompiled (task #44):
/// the splice would have bound `__molt_closure__` to the call's leading
/// function-value operand, so `index [__molt_closure__, 0]` subscripts a
/// function. `is_inlineable` must refuse it via the env-param marker.
fn closure_callee(name: &str) -> TirFunction {
    let mut f = TirFunction::new(
        name.into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::I64,
    );
    // The production lift sets param_names from the frontend params; mirror
    // that here (TirFunction::new defaults to "p0"/"p1", test-only). The
    // FIRST param is the captured-environment marker -> this is a closure.
    f.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.to_string(), "x".into()];
    let env = ValueId(0); // __molt_closure__
    let x = ValueId(1);
    let cell = f.fresh_value();
    let base = f.fresh_value();
    let sum = f.fresh_value();
    let entry = f.entry_block;
    let mut idx0a = AttrDict::new();
    idx0a.insert("value".into(), AttrValue::Int(0));
    let mut idx0b = AttrDict::new();
    idx0b.insert("value".into(), AttrValue::Int(0));
    let block = f.blocks.get_mut(&entry).unwrap();
    // cell = __molt_closure__[0]
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![env],
        results: vec![cell],
        attrs: idx0a,
        source_span: None,
    });
    // base = cell[0]
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![cell],
        results: vec![base],
        attrs: idx0b,
        source_span: None,
    });
    // sum = base + x
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![base, x],
        results: vec![sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![sum] };
    f.value_types.insert(cell, TirType::DynBox);
    f.value_types.insert(base, TirType::I64);
    f.value_types.insert(sum, TirType::I64);
    f
}

/// A const-returning leaf `fn k() -> 42`.
fn const_callee() -> TirFunction {
    let mut f = TirFunction::new("constfn".into(), vec![], TirType::I64);
    let v = f.fresh_value();
    let entry = f.entry_block;
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(42));
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v],
        attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![v] };
    f.value_types.insert(v, TirType::I64);
    f
}

/// A caller `fn g() { x = const(); y = x + 1; return y }` that calls the
/// const callee. The const arg list is empty; the result is `x`.
fn caller_calling_const(callee_name: &str) -> TirFunction {
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let call_res = g.fresh_value();
    let one = g.fresh_value();
    let y = g.fresh_value();
    let entry = g.entry_block;
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str(callee_name.to_string()));
    let mut one_attrs = AttrDict::new();
    one_attrs.insert("value".into(), AttrValue::Int(1));
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![call_res],
        attrs: call_attrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![one],
        attrs: one_attrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![call_res, one],
        results: vec![y],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![y] };
    g
}

fn module(funcs: Vec<TirFunction>) -> TirModule {
    TirModule {
        name: "m".into(),
        functions: funcs,
    }
}

fn analysis(m: &TirModule) -> (CallGraph, ModuleSummaries) {
    let cg = CallGraph::build(m);
    let sm = ModuleSummaries::compute(m, &cg);
    (cg, sm)
}

/// An **observation-only** callee `fn obs(a) -> a` shaped like real lowered
/// TIR: an entry block carrying a `CheckException` (handler label
/// `exc_label`) that, on a pending exception, routes to a void exception-exit
/// block (`ret_void`, reached only via the exception edge); the normal path
/// branches to a return block that yields the parameter. `has_exception_handling`
/// is set (the `CheckException` would set it during lift) but there is NO
/// handler region.
fn observation_callee_with_type(name: &str, exc_label: i64, ty: TirType) -> TirFunction {
    let mut f = TirFunction::new(name.into(), vec![ty.clone()], ty.clone());
    f.has_exception_handling = true;
    let a = ValueId(0);
    let normal = f.fresh_block();
    let exc_exit = f.fresh_block();
    let entry = f.entry_block;
    {
        let mut ce_attrs = AttrDict::new();
        ce_attrs.insert("value".into(), AttrValue::Int(exc_label));
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: ce_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Branch {
            target: normal,
            args: vec![],
        };
    }
    f.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![a] },
        },
    );
    f.blocks.insert(
        exc_exit,
        TirBlock {
            id: exc_exit,
            args: vec![],
            ops: vec![],
            // ret_void — propagate the pending flag.
            terminator: Terminator::Return { values: vec![] },
        },
    );
    // The exception edge resolves through label_id_map: the exit block carries
    // the handler label the entry's CheckException references.
    f.label_id_map.insert(exc_exit.0, exc_label);
    f.value_types.insert(a, ty);
    f
}

fn observation_callee(name: &str, exc_label: i64) -> TirFunction {
    observation_callee_with_type(name, exc_label, TirType::I64)
}

/// A caller `fn c() { r = obs(5); <observe>; return r }` that calls an
/// observation-only callee for a value, with its OWN post-call
/// `CheckException` (handler label `caller_label`, resolving to the caller's
/// own void exception-exit block). The caller's label deliberately COLLIDES
/// numerically with the callee's exception label so the clone's fresh-label
/// remap is exercised.
fn caller_calling_obs_with_label(name: &str, callee_name: &str, caller_label: i64) -> TirFunction {
    caller_calling_obs_with_label_and_type(name, callee_name, caller_label, TirType::I64)
}

fn caller_calling_obs_with_label_and_type(
    name: &str,
    callee_name: &str,
    caller_label: i64,
    ty: TirType,
) -> TirFunction {
    let mut c = TirFunction::new(name.into(), vec![], ty.clone());
    c.has_exception_handling = true;
    let arg = c.fresh_value();
    let call_res = c.fresh_value();
    let caller_exit = c.fresh_block();
    let entry = c.entry_block;
    {
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str(callee_name.to_string()));
        let mut ce_attrs = AttrDict::new();
        ce_attrs.insert("value".into(), AttrValue::Int(caller_label));
        let block = c.blocks.get_mut(&entry).unwrap();
        block.ops.push(dead_placeholder_const_for_type(&ty, arg));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![arg],
            results: vec![call_res],
            attrs: call_attrs,
            source_span: None,
        });
        // The caller's own post-call exception observation.
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: ce_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return {
            values: vec![call_res],
        };
    }
    c.blocks.insert(
        caller_exit,
        TirBlock {
            id: caller_exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    c.label_id_map.insert(caller_exit.0, caller_label);
    c.value_types.insert(arg, ty.clone());
    c.value_types.insert(call_res, ty);
    c
}

/// Convenience: caller with a non-colliding label.
fn caller_calling_obs(name: &str, callee_name: &str) -> TirFunction {
    caller_calling_obs_with_label(name, callee_name, 99)
}

// -- (a) clone + remap primitives ----------------------------------------

#[test]
fn clone_produces_disjoint_ids() {
    let callee = add_callee();
    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    // Two argument values already live in the caller.
    let a = caller.fresh_value();
    let b = caller.fresh_value();
    let before_next_value = caller.next_value;
    let before_next_block = caller.next_block;

    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);

    // The clone minted fresh value + block ids (the add result is fresh).
    assert!(caller.next_value > before_next_value, "value ids advanced");
    assert!(caller.next_block > before_next_block, "block ids advanced");
    // The cloned entry block exists and has NO args (params bound to a, b).
    let entry = &caller.blocks[&cloned.entry];
    assert!(
        entry.args.is_empty(),
        "cloned entry has no args (params bound)"
    );
    // The cloned Add uses the caller's arg values directly (a, b).
    let add = &entry.ops[0];
    assert_eq!(add.opcode, OpCode::Add);
    assert_eq!(add.operands, vec![a, b], "params bound directly to args");
    // The cloned add result is a fresh id, disjoint from a/b.
    assert!(add.results[0] != a && add.results[0] != b);
}

#[test]
fn clone_entry_has_empty_args() {
    let callee = add_callee();
    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    let a = caller.fresh_value();
    let b = caller.fresh_value();
    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);
    assert!(caller.blocks[&cloned.entry].args.is_empty());
}

#[test]
fn clone_transfers_all_loop_metadata() {
    // A callee with a header block carrying every loop-metadata kind.
    let mut callee = TirFunction::new("loopfn".into(), vec![], TirType::None);
    let header = callee.fresh_block();
    let end = callee.fresh_block();
    let cond = callee.fresh_block();
    // Give the entry a branch into the header; header/end/cond are trivial
    // blocks so the clone walk has them to remap.
    for bid in [header, end, cond] {
        callee.blocks.insert(
            bid,
            TirBlock {
                id: bid,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: header,
        args: vec![],
    };
    // Now wire all four loop maps + a label.
    callee.loop_roles.insert(header, LoopRole::LoopHeader);
    callee.loop_roles.insert(end, LoopRole::LoopEnd);
    callee.loop_pairs.insert(header, end);
    callee
        .loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfTrue);
    callee.loop_cond_blocks.insert(header, cond);
    callee.label_id_map.insert(header.0, 7);

    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[]);

    // All four maps + label_id_map must have one remapped entry each.
    assert_eq!(caller.loop_roles.len(), 2, "loop_roles transferred");
    assert_eq!(caller.loop_pairs.len(), 1, "loop_pairs transferred");
    assert_eq!(
        caller.loop_break_kinds.len(),
        1,
        "loop_break_kinds transferred"
    );
    assert_eq!(
        caller.loop_cond_blocks.len(),
        1,
        "loop_cond_blocks transferred"
    );
    assert_eq!(caller.label_id_map.len(), 1, "label_id_map transferred");
    // None of the transferred keys are the callee's original ids — they were
    // remapped to fresh caller block ids.
    assert!(!caller.loop_roles.contains_key(&header));
    assert!(!caller.loop_pairs.contains_key(&header));
    // The cloned entry is a fresh block (not the callee's BlockId(0)).
    assert!(cloned.entry != callee.entry_block || caller.next_block > callee.next_block);
}

// -- (b) splice ----------------------------------------------------------

#[test]
fn splice_removes_call_and_passes_verify() {
    let callee = const_callee();
    let mut caller = caller_calling_const("constfn");
    let site = collect_call_sites(&caller, &["constfn".to_string()]);
    assert_eq!(site.len(), 1);
    let did = splice_call_site(&mut caller, &callee, &site[0]);
    assert!(did, "splice succeeded");
    // No Call op remains anywhere.
    let remaining_calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(remaining_calls, 0, "the Call was eliminated");
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
}

#[test]
fn splice_void_return() {
    // Callee returns nothing; caller calls it for effect.
    let mut callee = TirFunction::new("eff".into(), vec![], TirType::None);
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new("g".into(), vec![], TirType::None);
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("eff".into()));
    let centry = caller.entry_block;
    let block = caller.blocks.get_mut(&centry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    let sites = collect_call_sites(&caller, &["eff".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(splice_call_site(&mut caller, &callee, &sites[0]));
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("void-splice invalid: {e:?}"));
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0);
}

#[test]
fn refcount_guard_refuses_arg_incref() {
    // Caller: IncRef(arg); call f(arg). The guard must refuse the splice.
    let mut callee = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let centry = callee.entry_block;
    callee.blocks.get_mut(&centry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new("g".into(), vec![TirType::DynBox], TirType::None);
    let arg = ValueId(0); // the caller's param
    let entry = caller.entry_block;
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("f".into()));
    let block = caller.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IncRef,
        operands: vec![arg],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![arg],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    let sites = collect_call_sites(&caller, &["f".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(
        !splice_call_site(&mut caller, &callee, &sites[0]),
        "refcount guard must refuse a site with arg IncRef before the call"
    );
    // The call survives intact.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 1, "refused site keeps its call");
}

// -- is_inlineable gates -------------------------------------------------

#[test]
fn recursive_not_inlined() {
    // f calls f → recursive.
    let mut f = TirFunction::new("f".into(), vec![], TirType::None);
    let entry = f.entry_block;
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("f".into()));
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn too_large_not_inlined() {
    // A callee with op_count > budget.
    let mut f = TirFunction::new("big".into(), vec![], TirType::I64);
    let entry = f.entry_block;
    let tti = TargetInfo::native_release_fast();
    let budget = tti.inline_budget("big");
    // Allocate value ids first (avoid overlapping borrows).
    let vals: Vec<ValueId> = (0..budget + 5).map(|_| f.fresh_value()).collect();
    let block = f.blocks.get_mut(&entry).unwrap();
    for v in &vals {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![*v],
            attrs,
            source_span: None,
        });
    }
    block.terminator = Terminator::Return {
        values: vec![vals[0]],
    };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "callee over budget is not inlinable"
    );
}

#[test]
fn generator_not_inlined() {
    let mut f = TirFunction::new("gen".into(), vec![], TirType::None);
    let entry = f.entry_block;
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Yield,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn entry_predecessor_callee_not_inlined() {
    // A callee whose entry block is a branch target (a back-edge to entry)
    // cannot be spliced by the direct-param-binding model — refuse it.
    let mut f = TirFunction::new("looper".into(), vec![], TirType::None);
    let body = f.fresh_block();
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            // body branches BACK to the entry → entry has a predecessor.
            terminator: Terminator::Branch {
                target: f.entry_block,
                args: vec![],
            },
        },
    );
    let entry = f.entry_block;
    f.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: body,
        args: vec![],
    };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "callee with entry-block predecessor is not inlinable this arc"
    );
}

#[test]
fn handler_bearing_callee_not_inlined() {
    // A callee with a REAL exception handler region (TryStart/TryEnd) is
    // excluded — splicing across a handler boundary needs handler-label
    // re-targeting this arc does not perform.
    let mut f = TirFunction::new("guarded".into(), vec![], TirType::None);
    f.has_exception_handling = true;
    let entry = f.entry_block;
    {
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryEnd,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
    }
    assert!(f.has_exception_handlers(), "TryStart/TryEnd => handlers");
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn observation_only_callee_is_inlineable() {
    // An OBSERVATION-only callee (CheckException, no handler region) IS
    // inlinable even though `has_exception_handling` is set: it has no real
    // handler, so `has_exception_handlers()` is false.
    let callee = observation_callee("obs", 3);
    assert!(
        callee.has_exception_handling,
        "CheckException sets has_exception_handling"
    );
    assert!(
        !callee.has_exception_handlers(),
        "no TryStart/TryEnd/StateBlock => no handler region"
    );
    let caller = caller_calling_obs("c", "obs");
    let m = module(vec![callee, caller]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let obs = m.functions.iter().find(|f| f.name == "obs").unwrap();
    assert!(
        is_inlineable(obs, &cg, &sm, &tti),
        "observation-only callee is inlinable"
    );
}

#[test]
fn closure_callee_not_inlined() {
    // task #44: a closure (first param == __molt_closure__) must NOT be
    // inlinable. The direct param->operand splice cannot bind the captured
    // env (it would bind the call's leading function-value operand instead),
    // so `is_inlineable` refuses it — conservative-correct exclusion.
    let callee = closure_callee("__main____add");
    assert!(
        is_closure(&callee),
        "first param is the env marker => closure"
    );
    let m = module(vec![callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "closure callee must be refused (task #44 miscompile gate)"
    );
}

#[test]
fn non_closure_same_arity_still_inlineable() {
    // The WIN must survive: a NON-closure 2-param callee (param_names do NOT
    // start with the env marker) is still inlinable. The closure gate keys on
    // the marker, not on arity, so a legitimate same-arity function is never
    // de-inlined by the fix.
    let callee = add_callee(); // params ["p0", "p1"] — not a closure
    assert!(
        !is_closure(&callee),
        "add_callee's first param is not the env marker"
    );
    let m = module(vec![callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "non-closure same-arity callee stays inlinable (perf win preserved)"
    );
}

#[test]
fn run_inliner_refuses_closure_call_site() {
    // End-to-end through the production chokepoint: a caller that calls a
    // closure must NOT have the call spliced away. The Call op survives and
    // the closure body is NOT cloned into the caller (no Index over the
    // function value), so the miscompile cannot occur.
    let callee = closure_callee("__main____add");
    // caller g(): r = __main____add(<func>, 10); return r.
    // Operands model the real call ABI [callee_value, arg0].
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let func_val = g.fresh_value();
    let ten = g.fresh_value();
    let res = g.fresh_value();
    let entry = g.entry_block;
    let mut fattrs = AttrDict::new();
    fattrs.insert("value".into(), AttrValue::Int(0)); // stand-in producer
    let mut tattrs = AttrDict::new();
    tattrs.insert("value".into(), AttrValue::Int(10));
    let mut call_attrs = AttrDict::new();
    call_attrs.insert(
        "s_value".into(),
        AttrValue::Str("__main____add".to_string()),
    );
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![func_val],
        attrs: fattrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![ten],
        attrs: tattrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![func_val, ten],
        results: vec![res],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![res] };
    g.value_types.insert(res, TirType::I64);

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 0, "closure call site is NOT inlined");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 1, "the Call op survives (closure not spliced)");
    // No Index op leaked into the caller (the closure body was not cloned).
    let indexes: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Index)
        .count();
    assert_eq!(indexes, 0, "closure body not cloned into caller");
}

// -- run_inliner end-to-end ----------------------------------------------

#[test]
fn run_inliner_inlines_const_call() {
    // g() { x = constfn(); return x + 1 }, constfn() = 42.
    // After inlining + re-running the pipeline, the Call is gone, the merged
    // function is valid SSA, and the callee's `const 42` now lives inside g
    // (the call boundary is eliminated). The downstream `const(42)+1 → 43`
    // arithmetic fold across the continuation block-argument is the
    // backend's / a future jump-threading pass's job — verified end-to-end
    // by the differential test, not asserted here (the current per-function
    // pipeline has no single-predecessor block-coalescing pass).
    let callee = const_callee();
    let caller = caller_calling_const("constfn");
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "one site inlined");
    assert_eq!(stats.functions_changed, 1, "g changed");
    // No Call op remains in g.
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "constfn call eliminated from g");
    // g is valid SSA after the pipeline re-run.
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after inlining: {e:?}"));
    // The inlined callee's `const 42` is now part of g's body.
    let has_const_42 = g.blocks.values().any(|b| {
        b.ops.iter().any(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
        })
    });
    assert!(has_const_42, "callee's const 42 inlined into g");
}

#[test]
fn clone_attrs_without_simple_names_drops_only_value_names() {
    // The strip helper drops `_simple_out` and `_simple_result_N` (the
    // collision-prone SimpleIR value-name annotations) but preserves every
    // other attribute verbatim (e.g. the call symbol or a const value).
    let mut attrs = AttrDict::new();
    attrs.insert("_simple_out".into(), AttrValue::Str("x".into()));
    attrs.insert("_simple_result_0".into(), AttrValue::Str("y".into()));
    attrs.insert("_simple_result_1".into(), AttrValue::Str("z".into()));
    attrs.insert("s_value".into(), AttrValue::Str("callee".into()));
    attrs.insert("value".into(), AttrValue::Int(7));
    let stripped = clone_attrs_without_simple_names(&attrs);
    assert!(!stripped.contains_key("_simple_out"), "_simple_out dropped");
    assert!(
        !stripped.contains_key("_simple_result_0"),
        "_simple_result_0 dropped"
    );
    assert!(
        !stripped.contains_key("_simple_result_1"),
        "_simple_result_1 dropped"
    );
    assert_eq!(
        stripped.get("s_value"),
        Some(&AttrValue::Str("callee".into())),
        "s_value preserved"
    );
    assert_eq!(
        stripped.get("value"),
        Some(&AttrValue::Int(7)),
        "value preserved"
    );
}

#[test]
fn inlined_ops_do_not_inherit_callee_simple_out_names() {
    // A callee whose op carries `_simple_out: "collide"` is inlined into a
    // caller that has its OWN op with the SAME `_simple_out: "collide"`. After
    // inlining, the name must appear on exactly ONE op (the caller's
    // original): the cloned callee op must have shed the name, so a name-keyed
    // container-dispatch lookup cannot resolve the inlined value to the
    // caller's kind. Before the strip, the merged body had two ops named
    // "collide" — a latent miscompile.
    let mut callee = TirFunction::new("c".into(), vec![], TirType::I64);
    let cv = callee.fresh_value();
    {
        let entry = callee.entry_block;
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
        let block = callee.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![cv],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![cv] };
    }
    callee.value_types.insert(cv, TirType::I64);

    // caller g(): own = const 9 (named "collide"); r = c(); return own.
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let own = g.fresh_value();
    let call_res = g.fresh_value();
    {
        let entry = g.entry_block;
        let mut own_attrs = AttrDict::new();
        own_attrs.insert("value".into(), AttrValue::Int(9));
        own_attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("c".into()));
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![own],
            attrs: own_attrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![call_res],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![own] };
    }
    g.value_types.insert(own, TirType::I64);
    g.value_types.insert(call_res, TirType::I64);

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "c() inlined into g");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let collide_count: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.attrs.get("_simple_out") == Some(&AttrValue::Str("collide".into())))
        .count();
    assert_eq!(
        collide_count, 1,
        "only the caller's own op keeps the name; the inlined op shed it"
    );
}

#[test]
fn run_inliner_inlines_add_call_with_args() {
    // g(p, q) { return addfn(p, q) }, addfn(a, b) = a + b.
    let callee = add_callee();
    let mut g = TirFunction::new("g".into(), vec![TirType::I64, TirType::I64], TirType::I64);
    let p = ValueId(0);
    let q = ValueId(1);
    let res = g.fresh_value();
    let entry = g.entry_block;
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("addfn".into()));
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![p, q],
        results: vec![res],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![res] };

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1);
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "addfn call eliminated");
    crate::tir::verify::verify_function(g).unwrap_or_else(|e| panic!("g invalid: {e:?}"));
    // The inlined body's Add (a+b with a=p, b=q) is present and uses the
    // caller's params directly.
    let add_uses_params = g.blocks.values().any(|b| {
        b.ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![p, q])
    });
    assert!(add_uses_params, "inlined add uses caller params directly");
}

#[test]
fn run_inliner_two_sites_same_block_both_inlined() {
    // g() { x = constfn(); y = constfn(); return x + y } — two calls to the
    // same inlinable leaf in one block. The reverse-order driver must splice
    // BOTH (a refused/early site must not block the other). After inlining,
    // zero Call ops remain and SSA is valid.
    let callee = const_callee();
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let x = g.fresh_value();
    let y = g.fresh_value();
    let sum = g.fresh_value();
    let entry = g.entry_block;
    let mk_call = |name: &str, out: ValueId| {
        let mut a = AttrDict::new();
        a.insert("s_value".into(), AttrValue::Str(name.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![out],
            attrs: a,
            source_span: None,
        }
    };
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(mk_call("constfn", x));
    block.ops.push(mk_call("constfn", y));
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![x, y],
        results: vec![sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![sum] };

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 2, "both call sites inlined");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "both constfn calls eliminated");
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after 2-site inlining: {e:?}"));
    // Two distinct const-42 ops now live in g (one per inlined site).
    let const_42_count: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
        })
        .count();
    assert_eq!(
        const_42_count, 2,
        "each inlined site contributes a const 42"
    );
}

// -- (c) exception-observation inlining ----------------------------------

/// The set of every label value in `func`'s `label_id_map` plus every
/// exception-op `"value"` label, used to assert collision-freedom.
fn all_labels(func: &TirFunction) -> Vec<i64> {
    function_label_ids(func).into_iter().collect()
}

#[test]
fn splice_observation_callee_remaps_labels_collision_free() {
    // Callee exception label 3; caller ALSO uses label 3 (collision). After
    // splicing, the cloned exit block must carry a FRESH label (not 3), the
    // caller's original label 3 must survive, and no two blocks may share a
    // label value (which would make `exception_label_to_block` ambiguous and
    // emit duplicate `label N` ops in lower_to_simple — a miscompile).
    let callee = observation_callee("obs", 3);
    let mut caller = caller_calling_obs_with_label("c", "obs", 3);

    let sites = collect_call_sites(&caller, &["obs".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(splice_call_site(&mut caller, &callee, &sites[0]), "spliced");

    // No two blocks share a label value.
    let labels: Vec<i64> = caller.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        labels.len(),
        "every block label is distinct (no collision): {labels:?}"
    );
    // The caller's original label 3 survived.
    assert!(all_labels(&caller).contains(&3), "caller label 3 preserved");

    // Every cloned CheckException's handler label resolves to a block that
    // carries that exact label in label_id_map (the exception edge resolves).
    let label_to_block: std::collections::HashMap<i64, BlockId> = caller
        .label_id_map
        .iter()
        .map(|(b, l)| (*l, BlockId(*b)))
        .collect();
    for block in caller.blocks.values() {
        for op in &block.ops {
            if let Some(label) = exception_label_of(op) {
                assert!(
                    label_to_block.contains_key(&label),
                    "CheckException label {label} resolves to a block"
                );
            }
        }
    }
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
    // The Call is gone.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated");
}

#[test]
fn splice_void_exception_exit_pads_placeholder_for_continuation_type() {
    // The observation callee's exception-exit returns NO value, but the call
    // wants one. The splice must NOT refuse (that would re-dormant the inliner
    // on every value-returning observation callee) — it pads the continuation
    // arg with a representation-matched dead placeholder. The exit branch ends
    // up supplying exactly one continuation arg, and the merged fn verifies.
    for (idx, ty) in [TirType::I64, TirType::Bool, TirType::F64, TirType::DynBox]
        .into_iter()
        .enumerate()
    {
        let callee_name = format!("obs_{idx}");
        let caller_name = format!("c_{idx}");
        let callee = observation_callee_with_type(&callee_name, 30 + idx as i64, ty.clone());
        let mut caller =
            caller_calling_obs_with_label_and_type(&caller_name, &callee_name, 90, ty.clone());

        let sites = collect_call_sites(&caller, std::slice::from_ref(&callee_name));
        assert_eq!(sites.len(), 1);
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0]),
            "value-returning observation callee inlines (not refused) for {ty:?}"
        );
        crate::tir::verify::verify_function(&caller).unwrap_or_else(|e| {
            panic!("merged fn invalid SSA after placeholder pad for {ty:?}: {e:?}")
        });

        let mut placeholder_const_seen = false;
        for block in caller.blocks.values() {
            if let Terminator::Branch { args, .. } = &block.terminator {
                if args.len() == 1
                    && block
                        .ops
                        .last()
                        .map(|op| {
                            let expected = dead_placeholder_const_for_type(&ty, args[0]);
                            op.opcode == expected.opcode
                                && op.operands == expected.operands
                                && op.results == expected.results
                                && op.attrs == expected.attrs
                        })
                        .unwrap_or(false)
                {
                    placeholder_const_seen = true;
                }
            }
        }
        assert!(
            placeholder_const_seen,
            "the void exception-exit branch is padded with a {ty:?} placeholder const"
        );
    }
}

#[test]
fn run_inliner_inlines_observation_callee_end_to_end() {
    // End-to-end through run_inliner (clone + splice + per-function pipeline
    // re-run): a value-returning observation-only callee is inlined, the Call
    // is gone, and the merged caller is valid SSA with collision-free labels.
    let callee = observation_callee("obs", 3);
    let caller = caller_calling_obs_with_label("c", "obs", 3);
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "obs inlined into c");
    let c = m.functions.iter().find(|f| f.name == "c").unwrap();
    let calls: usize = c
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated from c");
    crate::tir::verify::verify_function(c)
        .unwrap_or_else(|e| panic!("c invalid after observation inlining: {e:?}"));
    // Labels remain collision-free after the pipeline re-run.
    let labels: Vec<i64> = c.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len(), "labels distinct: {labels:?}");
}
