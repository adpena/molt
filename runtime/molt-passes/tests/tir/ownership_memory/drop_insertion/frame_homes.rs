//! Frame homes own their bindings (frame-slot custody).
//!
//! A synchronous Python frame's homes own its bindings from the first store. A
//! home store consumes its operand (`[[consuming_kind]]`); its result, like a
//! home load's, is a view that holds no reference and stays valid until the
//! next write to its slot. DropInsertion moves a stored value's own reference
//! into the home when nothing reads the value afterwards, a `Transferred`
//! parameter the frontend stores at entry included: the store ends that
//! binding, so no compiled code releases it again, and the next store, a `del`
//! or the frame's exit releases it when CPython would. A block argument that
//! carries only views is a view. Nothing releases a view, and an operation that
//! takes one gets a retained reference. A heap view reaching a `Return` fails
//! the pass: the frame's exit has released its home by then, so the frontend
//! returns an owned capture taken before the exit. Each case runs concrete
//! paths through the reference-count model in `mod.rs`, whose homes follow the
//! spellings' runtime semantics, not the pass's facts.
//!
//! The cases read the frame overlay's generated declarations
//! (frame-slot-custody COORDINATION §3): the three consuming rows, the binding
//! views and `frame_home_take`'s owned result. `declarations_are_generated`
//! states that precondition on its own.

use super::*;

use molt_passes::tir::op_kinds_generated::{
    copy_kind_is_binding_view_table, copy_kind_mints_owned_value_table, kind_consumed_operand_table,
};
use molt_passes::tir::passes::{copy_prop, gvn, mem_gvn};

const TRANSFERRED: ParameterCustody = ParameterCustody::Transferred;

fn function(name: &str, parameters: &[ParameterCustody]) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox; parameters.len()],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.set_parameter_custody(parameters);
    func
}

fn parameter(func: &TirFunction, position: usize) -> ValueId {
    func.blocks[&func.entry_block].args[position].id
}

fn owned(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::DynBox);
    value
}

fn flag(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::Bool);
    value
}

fn int(value: i64, result: ValueId) -> TirOp {
    let mut constant = op(OpCode::ConstInt, vec![], vec![result]);
    constant.attrs.insert("value".into(), AttrValue::Int(value));
    constant
}

/// An observable statement that reads `values`.
fn read(values: Vec<ValueId>) -> TirOp {
    op(OpCode::WarnStderr, values, vec![])
}

fn observe(label: i64) -> TirOp {
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check
}

/// The frontend's owned capture of a read, CPython's `LOAD_FAST` reference.
fn capture(read: ValueId, result: ValueId) -> TirOp {
    original_copy_with_operands("binding_alias", vec![read], vec![result])
}

/// A frame home access spelled `kind` on code slot `slot`.
fn home(kind: &str, slot: i64, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut access = original_copy_with_operands(kind, operands, results);
    access.attrs.insert("value".into(), AttrValue::Int(slot));
    access
}

/// `slot = value`: the home takes `value`'s reference; `view` names it.
fn store(slot: i64, value: ValueId, view: ValueId) -> TirOp {
    home("frame_home_store", slot, vec![value], vec![view])
}

/// A read of `slot`: `view` names what the home holds.
fn load(slot: i64, view: ValueId) -> TirOp {
    home("frame_home_load", slot, vec![], vec![view])
}

/// The frame's exit, which releases every home.
fn exit() -> TirOp {
    original_copy("trace_exit", vec![])
}

fn block(id: BlockId, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops,
        terminator,
    }
}

fn choose(cond: ValueId, then_block: BlockId, else_block: BlockId) -> Terminator {
    Terminator::CondBranch {
        cond,
        then_block,
        then_args: vec![],
        else_block,
        else_args: vec![],
    }
}

fn jump(target: BlockId, args: Vec<ValueId>) -> Terminator {
    Terminator::Branch { target, args }
}

fn done(values: Vec<ValueId>) -> Terminator {
    Terminator::Return { values }
}

fn body(func: &mut TirFunction, ops: Vec<TirOp>, terminator: Terminator) {
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops = ops;
    block.terminator = terminator;
}

fn argument(block: &mut TirBlock, id: ValueId) {
    block.args.push(TirValue {
        id,
        ty: TirType::DynBox,
    });
}

/// The frame overlay's declarations, generated: each store kind consumes its
/// one operand and returns a view, a load returns a view, and a take returns
/// an owned reference.
#[test]
fn declarations_are_generated() {
    for kind in [
        "frame_home_store",
        "frame_home_cell",
        "frame_home_private_cell",
    ] {
        assert_eq!(
            kind_consumed_operand_table(kind, 1),
            Some(0),
            "{kind} consumes its operand"
        );
        assert!(
            copy_kind_is_binding_view_table(kind),
            "{kind} returns a view"
        );
    }
    assert!(copy_kind_is_binding_view_table("frame_home_load"));
    assert!(copy_kind_mints_owned_value_table("frame_home_take"));
}

/// The frontend stores each parameter into its home at entry. The home takes
/// the argument's reference, which no compiled code releases again, and the
/// frame's exit releases it after the body's last statement. The same holds
/// for a cell, which a cell store moves into its home.
#[test]
fn parameter_stored_at_entry_moves_into_its_home() {
    for kind in [
        "frame_home_store",
        "frame_home_cell",
        "frame_home_private_cell",
    ] {
        let mut func = function("stores_its_parameter", &[TRANSFERRED]);
        let param = parameter(&func, 0);
        let view = owned(&mut func);
        body(
            &mut func,
            vec![home(kind, 0, vec![param], vec![view]), marker(), exit()],
            done(vec![]),
        );
        insert(&mut func);
        assert_eq!(
            (count_increfs(&func), count_decrefs(&func)),
            (0, 0),
            "{kind}: the home takes the argument's own reference"
        );
        assert_eq!(
            trace(&func, 0, &[]),
            [Event::Marker, Event::Freed(0)],
            "{kind}"
        );
    }
}

/// Rebinding a stored parameter releases the argument at the store, as
/// CPython's `STORE_FAST` does, before the statement that follows. A retain of
/// the parameter at its first store would keep it as a second owner until the
/// return, and its finalizer would run after the replacement's.
#[test]
fn rebinding_a_stored_parameter_releases_the_argument_at_the_store() {
    let mut func = function("rebinds_its_parameter", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, replacement, second) = (owned(&mut func), owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            store(0, param, first),
            marker(),
            produce(replacement),
            store(0, replacement, second),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(
        trace(&func, 0, &[]),
        [
            Event::Marker,
            Event::Freed(0),
            Event::Marker,
            Event::Freed(1)
        ]
    );
}

/// A failure before the entry store (a frame admission check) leaves the
/// argument with the activation: that exceptional exit releases it once, and
/// the normal path's home releases it at the frame's exit.
#[test]
fn failure_before_the_entry_store_releases_the_argument() {
    let mut func = function("fails_before_storing", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let view = owned(&mut func);
    let failed = func.fresh_block();
    func.label_id_map.insert(failed.0, 51);
    body(
        &mut func,
        vec![observe(51), store(0, param, view), marker(), exit()],
        done(vec![]),
    );
    func.blocks
        .insert(failed, block(failed, vec![], done(vec![])));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[true]), [Event::Freed(0)]);
    assert_eq!(trace(&func, 0, &[false]), [Event::Marker, Event::Freed(0)]);
}

/// A value the body still reads after storing it (an expression that outlives
/// the statement) keeps a reference of its own: the store takes a retained one,
/// and the value's own ends at its last read, before the home's.
#[test]
fn value_read_after_its_store_keeps_its_own_reference() {
    let mut func = function("reads_after_storing", &[]);
    let (value, view) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            produce(value),
            store(0, value, view),
            read(vec![value]),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 1));
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Marker, Event::Marker, Event::Freed(0)]
    );
}

/// A legacy STORE_VAR alias cannot keep the parameter's old SSA owner after
/// the home has become the Python binding's sole owner.
#[test]
#[should_panic(expected = "stale Python binding custody")]
fn cached_parameter_read_after_home_store_is_refused() {
    let mut func = function("cached_parameter_after_store", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (cached, view) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            original_store_var("parameter", param, cached),
            store(0, param, view),
            read(vec![cached]),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
}

#[test]
#[should_panic(expected = "stale Python binding custody")]
fn obsolete_parameter_release_after_home_store_is_refused() {
    let mut func = function("parameter_release_after_store", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let view = owned(&mut func);
    body(
        &mut func,
        vec![
            store(0, param, view),
            op(OpCode::DelBoundary, vec![param], vec![]),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
}

#[test]
#[should_panic(expected = "stale Python binding custody")]
fn deferred_named_owner_stored_into_a_home_is_refused() {
    let mut func = function("deferred_owner_after_store", &[]);
    let (value, view) = (owned(&mut func), owned(&mut func));
    let mut producer = produce(value);
    producer
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    body(
        &mut func,
        vec![producer, store(0, value, view), marker(), exit()],
        done(vec![]),
    );
    insert(&mut func);
}

/// Source deletion remains binding provenance after its physical normalization.
#[test]
#[should_panic(expected = "source_binding_release=true")]
fn unmarked_source_binding_cannot_survive_its_home_store() {
    let mut func = function("source_delete_after_store", &[]);
    let (value, view) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            produce(value),
            store(0, value, view),
            op(OpCode::DelBoundary, vec![value], vec![]),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
}

/// A physical release belongs to the expression's independent reference. Each
/// home takes its own reference, so its view survives that expression release.
#[test]
fn explicitly_released_expression_is_not_a_stale_python_binding() {
    for kind in [
        "frame_home_store",
        "frame_home_cell",
        "frame_home_private_cell",
    ] {
        let mut func = function("explicit_expression_home", &[]);
        let (value, view) = (owned(&mut func), owned(&mut func));
        body(
            &mut func,
            vec![
                produce(value),
                home(kind, 0, vec![value], vec![view]),
                op(OpCode::DecRef, vec![value], vec![]),
                read(vec![view]),
                exit(),
            ],
            done(vec![]),
        );
        insert(&mut func);
        assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 1));
        assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
    }
}

/// A region MatchRef and the handler's Python name have separate lifetimes.
/// The region pop releases the former; the home still keeps its view valid.
#[test]
fn handler_match_reference_keeps_region_custody_when_stored_in_a_home() {
    let mut func = function("handler_binding_and_match_reference", &[]);
    let handler = func.fresh_block();
    let (matched, view) = (owned(&mut func), owned(&mut func));
    func.label_id_map.insert(handler.0, 4);
    body(
        &mut func,
        vec![try_start(4), observe(4), exit()],
        done(vec![]),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![
                original_copy("exception_last_pending", vec![matched]),
                original_copy("exception_clear", vec![]),
                store(0, matched, view),
                read(vec![matched]),
                original_copy("exception_pop", vec![]),
                read(vec![view]),
                exit(),
            ],
            done(vec![]),
        ),
    );
    insert(&mut func);
    let ops = &func.blocks[&handler].ops;
    let pop = ops.iter().position(|op| {
        matches!(op.attrs.get("_original_kind"), Some(AttrValue::Str(kind)) if kind == "exception_pop")
    }).unwrap();
    assert_eq!(ops[pop + 1].opcode, OpCode::DecRef);
    assert_eq!(ops[pop + 1].operands, [matched]);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 1));
    assert_eq!(
        trace(&func, 0, &[true]),
        [Event::Marker, Event::Marker, Event::Freed(0)]
    );
    assert!(trace(&func, 0, &[false]).is_empty());
}

/// Explicitly held temporaries remain independent when different definitions
/// arrive through a join. The receiver's physical release is not a local name.
#[test]
fn joined_explicit_temporary_keeps_its_reference_across_a_home_store() {
    let mut func = function("joined_explicit_temporary_home", &[]);
    let (left_value, right_value, joined, view) = (
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
    );
    let choose_left = flag(&mut func);
    let (left, right, join) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![int(1, choose_left)],
        choose(choose_left, left, right),
    );
    func.blocks.insert(
        left,
        block(
            left,
            vec![produce(left_value)],
            jump(join, vec![left_value]),
        ),
    );
    func.blocks.insert(
        right,
        block(
            right,
            vec![produce(right_value)],
            jump(join, vec![right_value]),
        ),
    );
    let mut merge = block(
        join,
        vec![
            store(0, joined, view),
            op(OpCode::DecRef, vec![joined], vec![]),
            read(vec![view]),
            exit(),
        ],
        done(vec![]),
    );
    argument(&mut merge, joined);
    func.blocks.insert(join, merge);
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 1));
    for choices in [[true], [false]] {
        assert_eq!(trace(&func, 0, &choices), [Event::Marker, Event::Freed(0)]);
    }
}

/// A phi carrier inherits an explicit reference lifetime even when its own SSA
/// name has no DecRef yet. The old holder releases on the sibling path; this
/// path carries its obligation until the return boundary.
#[test]
fn explicit_reference_carrier_keeps_its_boundary_across_a_home_store() {
    let mut func = function("explicit_carrier_home", &[]);
    let (value, carried, view) = (owned(&mut func), owned(&mut func), owned(&mut func));
    let cond = flag(&mut func);
    let (release, keep) = (func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![produce(value), int(1, cond)],
        Terminator::CondBranch {
            cond,
            then_block: release,
            then_args: vec![],
            else_block: keep,
            else_args: vec![value],
        },
    );
    func.blocks.insert(
        release,
        block(
            release,
            vec![op(OpCode::DecRef, vec![value], vec![]), marker(), exit()],
            done(vec![]),
        ),
    );
    let mut carried_path = block(
        keep,
        vec![
            store(0, carried, view),
            home("frame_home_clear", 0, vec![], vec![]),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    argument(&mut carried_path, carried);
    func.blocks.insert(keep, carried_path);
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 2));
    assert_eq!(trace(&func, 0, &[true]), [Event::Freed(0), Event::Marker]);
    assert_eq!(trace(&func, 0, &[false]), [Event::Marker, Event::Freed(0)]);
}

/// Binding provenance moves along the same canonical arcs as reference custody.
#[test]
#[should_panic(expected = "parameter=true")]
fn carried_parameter_owner_cannot_survive_a_home_store() {
    let mut func = function("carried_parameter_home", &[TRANSFERRED]);
    let value = parameter(&func, 0);
    let (carried, view) = (owned(&mut func), owned(&mut func));
    let join = func.fresh_block();
    body(&mut func, vec![], jump(join, vec![value]));
    let mut merge = block(
        join,
        vec![store(0, carried, view), read(vec![carried]), exit()],
        done(vec![]),
    );
    argument(&mut merge, carried);
    func.blocks.insert(join, merge);
    insert(&mut func);
}

/// Silently deleting this boundary would keep the home bound. Only the home
/// clear operation can implement deletion of a home-backed Python name.
#[test]
#[should_panic(expected = "DelBoundary of binding view")]
fn deleting_a_view_instead_of_its_home_is_refused() {
    let mut func = function("deletes_view_not_home", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let view = owned(&mut func);
    body(
        &mut func,
        vec![
            store(0, param, view),
            op(OpCode::DelBoundary, vec![view], vec![]),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
}

/// `a = b = f()` retains the expression for its second store. That expression
/// is not a stale Python binding owner; both homes own it after the stores.
#[test]
fn chained_assignment_transfers_an_independent_expression() {
    let mut func = function("chained_assignment", &[]);
    let (value, first, second) = (owned(&mut func), owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            produce(value),
            store(0, value, first),
            store(1, value, second),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 0));
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// In `[x := A()]`, the expression reference survives its home store until
/// list construction absorbs it. Its statement release is not binding custody.
#[test]
fn walrus_expression_keeps_its_statement_reference_through_absorption() {
    let mut func = function("walrus_absorption", &[]);
    let (value, view, list) = (owned(&mut func), owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            finalizer_object(value),
            store(0, value, view),
            original_copy_with_operands("list_new", vec![value], vec![list]),
            read(vec![list]),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    let ops = &func.blocks[&func.entry_block].ops;
    let absorb = ops.iter().position(|op| op.results == [list]).unwrap();
    assert!(
        ops[..absorb]
            .iter()
            .any(|op| { op.opcode == OpCode::IncRef && op.operands == [value] })
    );
    assert_eq!(ops[absorb + 1].opcode, OpCode::DecRef);
    assert_eq!(ops[absorb + 1].operands, [value]);
    assert_eq!(
        ops.iter()
            .filter(|op| { op.opcode == OpCode::DecRef && op.operands == [value] })
            .count(),
        1
    );
}

/// A generic consuming ABI operation takes a reference, not the caller's
/// Python binding. The caller's parameter must survive to its own boundary.
#[test]
fn generic_consumption_preserves_the_callers_binding() {
    let mut func = function(
        "consumes_without_unbinding",
        &[ParameterCustody::Borrowed, TRANSFERRED],
    );
    let callable = parameter(&func, 0);
    let builder = parameter(&func, 1);
    let call = call_bind(callable, builder, vec![]);
    body(&mut func, vec![call, marker()], done(vec![]));
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 1));
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(1)]);
}

/// A raw binding holds no reference: its store takes none, and the pass places
/// no reference operation.
#[test]
fn raw_binding_moves_no_reference() {
    let mut func = function("stores_an_int", &[]);
    let (five, view) = (func.fresh_value(), func.fresh_value());
    func.value_types.insert(five, TirType::I64);
    body(
        &mut func,
        vec![int(5, five), store(0, five, view), read(vec![view]), exit()],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(trace(&func, 0, &[]), [Event::Marker]);
}

/// `x = A()` before a loop that rebinds `x`, and a read of `x` after it. The
/// loop header's argument carries a store view on every arc, so it is a view:
/// no arc retains it, nothing releases it, and each store releases the binding
/// it replaces when CPython would.
#[test]
fn views_joined_around_a_loop_hold_no_reference() {
    let mut func = function("loop_rebinds_a_local", &[]);
    let (initial, first) = (owned(&mut func), owned(&mut func));
    let (next, rebound, current) = (owned(&mut func), owned(&mut func), owned(&mut func));
    let more = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let more_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (header, loop_body, after) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    body(
        &mut func,
        vec![produce(initial), store(0, initial, first)],
        jump(header, vec![first]),
    );
    let mut head = block(
        header,
        vec![op(OpCode::Copy, vec![more_input], vec![more])],
        choose(more, loop_body, after),
    );
    argument(&mut head, current);
    func.blocks.insert(header, head);
    func.blocks.insert(
        loop_body,
        block(
            loop_body,
            vec![produce(next), store(0, next, rebound)],
            jump(header, vec![rebound]),
        ),
    );
    func.blocks.insert(
        after,
        block(
            after,
            vec![read(vec![current]), marker(), exit()],
            done(vec![]),
        ),
    );
    insert(&mut func);
    assert_eq!(
        (count_increfs(&func), count_decrefs(&func)),
        (0, 0),
        "no arc retains a view, and nothing releases one"
    );
    for iterations in 0..3 {
        let mut choices = vec![true; iterations];
        choices.push(false);
        let mut expected: Vec<Event> = (0..iterations).map(Event::Freed).collect();
        expected.extend([Event::Marker, Event::Marker, Event::Freed(iterations)]);
        assert_eq!(
            trace(&func, 0, &choices),
            expected,
            "{iterations} iterations"
        );
    }
}

/// A join of a view and an owned temporary is an owner, as any mixed join is:
/// the view's arc retains, the temporary moves in, and the join's reference
/// ends at its last read on either arm.
#[test]
fn join_of_a_view_and_an_owner_owns_its_value() {
    let mut func = function("joins_a_view_and_a_temporary", &[]);
    let (value, view, other, joined) = (
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
    );
    let cond = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let cond_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (left, right, join) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![
            produce(value),
            store(0, value, view),
            op(OpCode::Copy, vec![cond_input], vec![cond]),
        ],
        choose(cond, left, right),
    );
    func.blocks
        .insert(left, block(left, vec![], jump(join, vec![view])));
    func.blocks.insert(
        right,
        block(right, vec![produce(other)], jump(join, vec![other])),
    );
    let mut merge = block(
        join,
        vec![read(vec![joined]), marker(), exit()],
        done(vec![]),
    );
    argument(&mut merge, joined);
    func.blocks.insert(join, merge);
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[true]),
        [Event::Marker, Event::Marker, Event::Freed(0)]
    );
    assert_eq!(
        trace(&func, 0, &[false]),
        [
            Event::Marker,
            Event::Freed(1),
            Event::Marker,
            Event::Freed(0)
        ]
    );
}

/// A call that adopts a view receives a retained reference; the home keeps its
/// own until the frame's exit.
#[test]
fn view_passed_to_an_adopting_call_is_retained() {
    let mut func = function("passes_a_local", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, current) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            store(0, param, first),
            load(0, current),
            transfer(vec![current]),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (1, 0));
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// `return x`: the frontend captures the view before the frame's exit, the
/// exit releases the home, and the return moves the capture's reference to the
/// caller with no publication retain.
#[test]
fn view_captured_before_frame_exit_is_returned_owned() {
    let mut func = function("returns_a_local", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, current, captured) = (owned(&mut func), owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            store(0, param, first),
            load(0, current),
            capture(current, captured),
            exit(),
        ],
        done(vec![captured]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(trace(&func, 0, &[]), Vec::<Event>::new());
}

/// Why a returned view needs the frontend's capture. A publication retain can
/// sit only at the terminator, after the frame's exit, and there the home's
/// only reference is gone: this body, with that retain placed by hand, fails
/// the reference-count model at the retain.
#[test]
fn retain_after_frame_exit_names_a_released_view() {
    let mut func = function("retains_after_exit", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, current) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            store(0, param, first),
            load(0, current),
            exit(),
            op(OpCode::IncRef, vec![current], vec![]),
        ],
        done(vec![current]),
    );
    let defect = execute(&func, 0, &[]).expect_err("the exit frees the home's only reference");
    assert!(defect.contains("released"), "{defect}");
}

/// The pass never repairs a return, and never emits that retain: a heap view
/// returned without the frontend's capture fails DropInsertion, before any
/// backend sees the body. No verifier after DropInsertion knows views, and the
/// SimpleIR frame-lifecycle rule checks only that `trace_exit` precedes the
/// return. The capture is the frontend's obligation (pass INTERFACE §9.13).
#[test]
#[should_panic(expected = "return of frame binding view")]
fn view_returned_without_a_capture_is_refused() {
    let mut func = function("returns_an_uncaptured_local", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, current) = (owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![store(0, param, first), load(0, current), exit()],
        done(vec![current]),
    );
    insert(&mut func);
}

/// A loop-carried view returned after the loop is a view too, through its
/// block argument, and is refused the same way.
#[test]
#[should_panic(expected = "return of frame binding view")]
fn joined_view_returned_without_a_capture_is_refused() {
    let mut func = function("returns_a_loop_local", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, rebound, current) = (owned(&mut func), owned(&mut func), owned(&mut func));
    let more = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let more_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (header, loop_body, after) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    body(
        &mut func,
        vec![store(0, param, first)],
        jump(header, vec![first]),
    );
    let mut head = block(
        header,
        vec![op(OpCode::Copy, vec![more_input], vec![more])],
        choose(more, loop_body, after),
    );
    argument(&mut head, current);
    func.blocks.insert(header, head);
    func.blocks.insert(
        loop_body,
        block(
            loop_body,
            vec![load(0, rebound)],
            jump(header, vec![rebound]),
        ),
    );
    func.blocks
        .insert(after, block(after, vec![exit()], done(vec![current])));
    insert(&mut func);
}

/// A comprehension's isolated binding (PEP 709): the take moves the outer
/// binding out and owns it, the inner store fills the emptied home, and the
/// restore moves the saved binding back, releasing the inner one there.
#[test]
fn taken_binding_is_owned_until_its_restore() {
    let mut func = function("isolates_a_comprehension_binding", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, saved, inner, inner_view, restored) = (
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
        owned(&mut func),
    );
    body(
        &mut func,
        vec![
            store(0, param, first),
            home("frame_home_take", 0, vec![], vec![saved]),
            produce(inner),
            store(0, inner, inner_view),
            store(0, saved, restored),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(1), Event::Marker, Event::Freed(0)]
    );
}

/// `del x` releases the binding at the statement, and the frame's exit finds
/// the home empty.
#[test]
fn del_releases_the_binding_at_the_statement() {
    let mut func = function("deletes_its_parameter", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let view = owned(&mut func);
    body(
        &mut func,
        vec![
            store(0, param, view),
            marker(),
            home("frame_home_clear", 0, vec![], vec![]),
            marker(),
            exit(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!((count_increfs(&func), count_decrefs(&func)), (0, 0));
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Marker, Event::Freed(0), Event::Marker]
    );
}

/// Two reads of one local with a statement between them, where a callback
/// could rebind it: value numbering, load forwarding and copy propagation
/// leave every home access in place, so each read observes the home.
#[test]
fn home_accesses_are_neither_numbered_nor_forwarded() {
    let mut func = function("reads_a_local_twice", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let (first, before, after) = (owned(&mut func), owned(&mut func), owned(&mut func));
    body(
        &mut func,
        vec![
            store(0, param, first),
            load(0, before),
            marker(),
            load(0, after),
            read(vec![first]),
            read(vec![before]),
            read(vec![after]),
            exit(),
        ],
        done(vec![]),
    );
    let printed = molt_passes::tir::printer::print_function(&func);
    gvn::run(&mut func, &mut AnalysisManager::new());
    mem_gvn::run(&mut func, &mut AnalysisManager::new());
    copy_prop::run(&mut func);
    assert_eq!(molt_passes::tir::printer::print_function(&func), printed);
}

/// Type refinement: a store's view takes the stored value's type, and a load,
/// which names whatever the home holds, infers nothing from the store.
#[test]
fn store_view_takes_its_operand_type_and_a_load_does_not() {
    let mut func = function("stores_then_reads_an_int", &[]);
    let (seven, view, loaded) = (func.fresh_value(), func.fresh_value(), func.fresh_value());
    func.value_types.insert(seven, TirType::I64);
    body(
        &mut func,
        vec![
            int(7, seven),
            store(0, seven, view),
            load(0, loaded),
            read(vec![view]),
            read(vec![loaded]),
            exit(),
        ],
        done(vec![]),
    );
    molt_passes::tir::type_refine::refine_types(&mut func);
    assert_eq!(func.value_types.get(&view), Some(&TirType::I64));
    assert_ne!(func.value_types.get(&loaded), Some(&TirType::I64));
}

/// The store owns its operand before its fallible boxed view is observed.
/// Its authored edge precedes TRY_END and later effects; cleanup releases the
/// home and the still-live independent parameter once on either continuation.
#[test]
fn store_failure_edge_preserves_handler_and_custody_before_later_effects() {
    use molt_passes::tir::exception_regions::{
        ExceptionBoundaryHandler, ExceptionOpPosition, ExceptionRegions,
    };
    use molt_passes::tir::passes::check_exception_elim;

    let mut func = function("home_store_failure", &[TRANSFERRED, TRANSFERRED]);
    let source = parameter(&func, 0);
    let independent = parameter(&func, 1);
    let view = owned(&mut func);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 51);
    let mut end = op(OpCode::TryEnd, vec![], vec![]);
    end.attrs.insert("value".into(), AttrValue::Int(51));
    body(
        &mut func,
        vec![
            try_start(51),
            store(0, source, view),
            observe(51),
            end.clone(),
            read(vec![view]),
            read(vec![independent]),
            exit(),
        ],
        done(vec![]),
    );
    func.blocks
        .insert(handler, block(handler, vec![end, exit()], done(vec![])));
    func.has_exception_handling = true;
    check_exception_elim::run(&mut func);
    assert_eq!(
        func.blocks[&func.entry_block].ops[2].opcode,
        OpCode::CheckException
    );
    let mut analysis = AnalysisManager::new();
    assert_eq!(
        analysis
            .get::<ExceptionRegions>(&func)
            .lexical_handler_before(ExceptionOpPosition {
                block: func.entry_block,
                op_index: 2,
            }),
        Ok(ExceptionBoundaryHandler::Labeled(51)),
    );
    insert(&mut func);
    let reads: Vec<_> = func.blocks[&func.entry_block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::WarnStderr)
        .map(|op| op.operands.clone())
        .collect();
    assert_eq!(
        reads,
        [vec![view], vec![independent]],
        "each live binding has its own explicit singleton diagnostic read"
    );
    for failed in [false, true] {
        let events = trace(&func, 0, &[failed]);
        assert_eq!(
            events
                .iter()
                .filter(|event| **event == Event::Marker)
                .count(),
            2 * usize::from(!failed),
            "both singleton reads execute only on the normal continuation"
        );
        for object in [0, 1] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| **event == Event::Freed(object))
                    .count(),
                1
            );
        }
    }
}
