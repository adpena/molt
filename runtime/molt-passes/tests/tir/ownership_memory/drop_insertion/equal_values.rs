//! Equal values are not equal owners.
//!
//! Value numbering, load forwarding, algebraic identities and tuple
//! scalarization rewrite an operation whose result equals an existing value
//! into a `Copy` of that value. The result held a reference of its own, and a
//! Python binding of it ends at its own boundary. Each case runs the real pass
//! and copy propagation, places RC with DropInsertion, and traces concrete paths
//! through the reference-count model in `mod.rs`. A replaced result folded into
//! its equal's reference fails the path: a `del` of one name frees what the
//! other still reads, or two boundaries release one reference twice. A raw
//! carrier holds no reference, and a type guard's result names its operand's,
//! so their copies stay transparent and add no owner.

use super::*;

use molt_passes::tir::passes::{canonicalize, copy_prop, deforestation, gvn, mem_gvn};

fn function(name: &str) -> TirFunction {
    TirFunction::new(
        name.into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    )
}

fn body(func: &mut TirFunction, ops: Vec<TirOp>, terminator: Terminator) {
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops = ops;
    block.terminator = terminator;
}

fn block(id: BlockId, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops,
        terminator,
    }
}

fn jump(target: BlockId, args: Vec<ValueId>) -> Terminator {
    Terminator::Branch { target, args }
}

fn done() -> Terminator {
    Terminator::Return { values: vec![] }
}

fn int(value: i64, result: ValueId) -> TirOp {
    let mut constant = op(OpCode::ConstInt, vec![], vec![result]);
    constant.attrs.insert("value".into(), AttrValue::Int(value));
    constant
}

/// `lhs + rhs`: on exact strings, value numbering merges equal ones.
fn concat(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
    op(OpCode::Add, vec![lhs, rhs], vec![result])
}

/// An observable statement that reads `values`.
fn read(values: Vec<ValueId>) -> TirOp {
    op(OpCode::WarnStderr, values, vec![])
}

/// The `del` of the binding `value`.
fn del(value: ValueId) -> TirOp {
    op(OpCode::DelBoundary, vec![value], vec![])
}

/// A no-op `str` type guard of `value`: `result` names `value`'s object and
/// holds no reference of its own.
fn guard(value: ValueId, result: ValueId) -> TirOp {
    let mut guard = op(OpCode::TypeGuard, vec![value], vec![result]);
    guard
        .attrs
        .insert("expected_type".into(), AttrValue::Str("str".into()));
    guard
}

/// Numbers values as the pipeline does: GVN, then copy propagation.
fn number(func: &mut TirFunction) {
    gvn::run(func, &mut AnalysisManager::new());
    copy_prop::run(func);
}

/// The value that `result` is an owned alias of, when an owned alias defines
/// it.
fn owned_alias_source(func: &TirFunction, result: ValueId) -> Option<ValueId> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|operation| operation.results == [result])
        .filter(|operation| {
            operation.opcode == OpCode::Copy
                && operation.attrs.get("_original_kind")
                    == Some(&AttrValue::Str("binding_alias".into()))
        })
        .map(|operation| operation.operands[0])
}

/// `a = s + s; b = s + s; del a; print(b)`. Value numbering replaces `b`'s
/// computation, and `b` keeps a reference of its own: the `del` ends `a`'s
/// only, and `b` lives to its read.
#[test]
fn numbered_binding_outlives_the_del_of_its_equal() {
    let mut func = function("del_then_read_equal");
    let (text, a, b) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    body(
        &mut func,
        vec![
            const_str(text),
            concat(text, text, a),
            concat(text, text, b),
            marker(),
            del(a),
            read(vec![b]),
        ],
        done(),
    );
    number(&mut func);
    assert_eq!(owned_alias_source(&func, b), Some(a));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [
            Event::Freed(0),
            Event::Marker,
            Event::Marker,
            Event::Freed(1)
        ]
    );
}

/// The replaced binding lives in a later block: `a` in the entry, `b = s + s`
/// and `del a` in the next block, and a read and `del` of `b` at the exit. The
/// first `del` ends `a`'s reference only, and the second ends `b`'s: two
/// boundaries, two references.
#[test]
fn numbered_binding_in_a_later_block_keeps_its_own_boundary() {
    let mut func = function("equal_bindings_across_blocks");
    let (text, a, b) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    let (next, exit) = (func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![const_str(text), concat(text, text, a)],
        jump(next, vec![]),
    );
    func.blocks.insert(
        next,
        block(
            next,
            vec![concat(text, text, b), del(a), marker()],
            jump(exit, vec![]),
        ),
    );
    func.blocks.insert(
        exit,
        block(exit, vec![read(vec![b]), del(b), marker()], done()),
    );
    number(&mut func);
    assert_eq!(owned_alias_source(&func, b), Some(a));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [
            Event::Freed(0),
            Event::Marker,
            Event::Marker,
            Event::Freed(1),
            Event::Marker
        ]
    );
}

/// `b = s + s` on one arm, which deletes `a`, and a join that reads whichever
/// binding its arm passed. On either path the joined value holds a reference of
/// its own and dies after its read.
#[test]
fn numbered_binding_joined_after_the_del_of_its_equal() {
    let mut func = function("equal_binding_joins");
    let (text, a, b) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    let (cond, joined) = (func.fresh_value(), func.fresh_value());
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let cond_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    func.value_types.insert(cond, TirType::Bool);
    let (left, right, join) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![
            const_str(text),
            concat(text, text, a),
            op(OpCode::Copy, vec![cond_input], vec![cond]),
        ],
        Terminator::CondBranch {
            cond,
            then_block: left,
            then_args: vec![],
            else_block: right,
            else_args: vec![],
        },
    );
    func.blocks.insert(
        left,
        block(
            left,
            vec![concat(text, text, b), del(a)],
            jump(join, vec![b]),
        ),
    );
    func.blocks
        .insert(right, block(right, vec![], jump(join, vec![a])));
    let mut merge = block(join, vec![marker(), read(vec![joined])], done());
    merge.args = vec![TirValue {
        id: joined,
        ty: TirType::DynBox,
    }];
    func.blocks.insert(join, merge);
    number(&mut func);
    assert_eq!(owned_alias_source(&func, b), Some(a));
    insert(&mut func);
    for taken in [true, false] {
        assert_eq!(
            trace(&func, 0, &[taken]),
            [
                Event::Freed(0),
                Event::Marker,
                Event::Marker,
                Event::Freed(1)
            ],
            "left arm taken: {taken}"
        );
    }
}

/// `f(a, b)` adopting both operands, with `b = s + s` numbered onto `a`, then
/// `del b` and a read of `a`. The call adopts one reference per position, the
/// `del` ends `b`'s, and `a` lives to its read.
#[test]
fn numbered_binding_passed_beside_its_equal_keeps_its_own_reference() {
    let mut func = function("equal_bindings_passed_together");
    let (text, a, b) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    let call = transfer(vec![a, b]);
    body(
        &mut func,
        vec![
            const_str(text),
            concat(text, text, a),
            concat(text, text, b),
            call,
            del(b),
            marker(),
            read(vec![a]),
        ],
        done(),
    );
    number(&mut func);
    assert_eq!(owned_alias_source(&func, b), Some(a));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [
            Event::Freed(0),
            Event::Marker,
            Event::Marker,
            Event::Freed(1)
        ]
    );
}

/// Raw carriers hold no reference: the numbered copy stays transparent for
/// copy propagation, and DropInsertion places no reference operation.
#[test]
fn numbered_raw_scalar_stays_a_transparent_copy() {
    let mut func = function("equal_raw_scalars");
    let (three, four) = (func.fresh_value(), func.fresh_value());
    let (a, b) = (func.fresh_value(), func.fresh_value());
    for value in [three, four, a, b] {
        func.value_types.insert(value, TirType::I64);
    }
    body(
        &mut func,
        vec![
            int(3, three),
            int(4, four),
            op(OpCode::Add, vec![three, four], vec![a]),
            op(OpCode::Add, vec![three, four], vec![b]),
            marker(),
            del(a),
            read(vec![b]),
        ],
        done(),
    );
    gvn::run(&mut func, &mut AnalysisManager::new());
    let copy = &func.blocks[&func.entry_block].ops[3];
    assert!(
        copy.opcode == OpCode::Copy && !copy.attrs.contains_key("_original_kind"),
        "{copy:?}"
    );
    copy_prop::run(&mut func);
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Marker]);
}

/// Two equal type guards of one string: value numbering replaces the second
/// with the first. Neither guard's result held a reference of its own; each
/// named the string's. The copy stays transparent, so the string is released
/// once, after the last read of either name, and no owner is added.
#[test]
fn numbered_type_guard_stays_a_transparent_alias() {
    let mut func = function("equal_type_guards");
    let (text, first, second) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    body(
        &mut func,
        vec![
            const_str(text),
            guard(text, first),
            guard(text, second),
            marker(),
            read(vec![second]),
        ],
        done(),
    );
    gvn::run(&mut func, &mut AnalysisManager::new());
    let copy = &func.blocks[&func.entry_block].ops[2];
    assert!(
        copy.opcode == OpCode::Copy
            && copy.operands == [first]
            && !copy.attrs.contains_key("_original_kind"),
        "{copy:?}"
    );
    copy_prop::run(&mut func);
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 1));
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Marker, Event::Marker, Event::Freed(0)]
    );
}

/// `r = x + 0` on an integer that may need a heap BigInt: canonicalization
/// rewrites `r` into `x`'s value, and `r` keeps a reference of its own through
/// the `del` of `x`.
#[test]
fn identity_result_outlives_the_del_of_its_operand() {
    let mut func = function("identity_outlives_del");
    let (big, zero) = (func.fresh_value(), func.fresh_value());
    let (x, r) = (func.fresh_value(), func.fresh_value());
    for constant in [big, zero] {
        func.value_types.insert(constant, TirType::I64);
    }
    body(
        &mut func,
        vec![
            int(1 << 40, big),
            int(0, zero),
            op(OpCode::Mul, vec![big, big], vec![x]),
            op(OpCode::Add, vec![x, zero], vec![r]),
            del(x),
            read(vec![r]),
        ],
        done(),
    );
    canonicalize::run(&mut func);
    copy_prop::run(&mut func);
    assert_eq!(owned_alias_source(&func, r), Some(x));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// `p, q = x, y; del x; print(p)`: scalarizing the tuple rewrites `p` into
/// `x`'s value, and `p` keeps a reference of its own through the `del` of `x`.
/// The unread `q` names nothing and stays a transparent copy.
#[test]
fn unpacked_binding_outlives_the_del_of_its_element() {
    let mut func = function("unpacked_outlives_del");
    let (x, y, pair) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    let (p, q) = (func.fresh_value(), func.fresh_value());
    body(
        &mut func,
        vec![
            const_str(x),
            const_str(y),
            op(OpCode::BuildTuple, vec![x, y], vec![pair]),
            op(OpCode::UnpackSequence, vec![pair], vec![p, q]),
            del(x),
            read(vec![p]),
        ],
        done(),
    );
    deforestation::run_tuple_scalarize(&mut func);
    copy_prop::run(&mut func);
    assert_eq!(owned_alias_source(&func, p), Some(x));
    assert_eq!(owned_alias_source(&func, q), None);
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(1), Event::Marker, Event::Freed(0)]
    );
}

/// `obj.f = s; r = obj.f; del s; print(r)`: forwarding the store rewrites the
/// load into `s`'s value, and `r` keeps the reference the load gave it through
/// the `del` of `s`, released after its read. A transparent copy would let the
/// `del` free what `r` reads, and a separate `IncRef` beside it would be a
/// reference no owner releases.
#[test]
fn forwarded_load_keeps_the_reference_the_load_gave_it() {
    let mut func = function("forwarded_load_owner");
    let (object, text, loaded) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    let mut allocation = op(OpCode::Alloc, vec![], vec![object]);
    allocation.attrs.insert("value".into(), AttrValue::Int(16));
    let mut store = op(OpCode::StoreAttr, vec![object, text], vec![]);
    store.attrs.insert("value".into(), AttrValue::Int(0));
    store
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    let mut load = op(OpCode::LoadAttr, vec![object], vec![loaded]);
    load.attrs.insert("value".into(), AttrValue::Int(0));
    load.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    body(
        &mut func,
        vec![
            allocation,
            const_str(text),
            store,
            load,
            del(text),
            read(vec![loaded]),
        ],
        done(),
    );
    mem_gvn::run(&mut func, &mut AnalysisManager::new());
    copy_prop::run(&mut func);
    assert_eq!(owned_alias_source(&func, loaded), Some(text));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(0), Event::Marker, Event::Freed(1)]
    );
}
