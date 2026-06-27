use super::call_sites::collect_call_sites;
use super::clone_body::{clone_attrs_without_simple_names, clone_function_body_with_fresh_ids};
use super::eligibility::is_closure;
use super::exception_labels::{exception_label_of, function_label_ids};
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

mod clone_body;
mod driver;
mod eligibility;
mod splice;
