//! Callee-owned parameters and the source call instructions that adopt their
//! arguments.
//!
//! A function whose parameter custody is `Transferred` owns that parameter
//! like a Python local: it releases it at the binding's boundary, not at its
//! last read. A return moves either the binding's own reference or, where frame
//! teardown could invalidate the binding first, the frontend's owned capture
//! of it. A source Python call
//! instruction adopts one reference per `Transferred` operand, on both of its
//! continuations: the caller moves a dead owner's own reference into the first
//! position naming it and retains one for every other position. Runtime helper
//! calls without source argument custody stay borrowed. The module inliner
//! keeps the same contract when it splices a call: the inlined activation
//! releases its frame bindings at each of its exits, a borrowed argument after
//! the whole body, and the result wherever the caller drops it. Each case runs
//! concrete paths through the reference-count model in `mod.rs`, which releases
//! adopted references inside the call. A missing retain, a release of a moved
//! owner, an early release of a Python binding, or a transferred parameter the
//! path never releases fails the path wherever the pass placed its operations.

use super::*;

use molt_passes::tir::passes::inliner::run_inliner;
use molt_passes::tir::passes::ip_summary::ModuleSummaries;
use molt_passes::tir::{CallGraph, TargetInfo, TirModule};

const BORROWED: ParameterCustody = ParameterCustody::Borrowed;
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

/// A named local: a Python-bound producer whose result a slot store holds.
fn bind_local(result: ValueId) -> [TirOp; 2] {
    let mut make = produce(result);
    make.attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    [
        make,
        original_copy_with_operands("store_var", vec![result], vec![]),
    ]
}

fn observe(label: i64) -> TirOp {
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check
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

fn jump(target: BlockId) -> Terminator {
    Terminator::Branch {
        target,
        args: vec![],
    }
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

/// An owner that nothing reads after the call moves its own reference into
/// it: the callee releases the object inside the call, and the caller places
/// no reference operation at all.
#[test]
fn dead_owner_moves_into_the_adopting_call() {
    let mut func = function("dead_owner_moves", &[]);
    let value = owned(&mut func);
    body(
        &mut func,
        vec![produce(value), transfer(vec![value]), marker()],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Freed(0), Event::Marker]);
    assert_eq!(
        (count_increfs(&func), count_decrefs(&func)),
        (0, 0),
        "a move costs no reference traffic"
    );
}

/// A root named at two adopted positions, directly or through a transparent
/// alias, owes two references: it moves one and retains the other.
#[test]
fn root_named_twice_owes_two_references() {
    for through_alias in [false, true] {
        let mut func = function("root_named_twice", &[]);
        let value = owned(&mut func);
        let mut ops = vec![produce(value)];
        let second = if through_alias {
            let alias = owned(&mut func);
            ops.push(op(OpCode::Copy, vec![value], vec![alias]));
            alias
        } else {
            value
        };
        ops.extend([transfer(vec![value, second]), marker()]);
        body(&mut func, ops, done(vec![]));
        insert(&mut func);
        assert_eq!(trace(&func, 0, &[]), [Event::Freed(0), Event::Marker]);
    }
}

/// An owner the caller still reads keeps its own reference: the call adopts a
/// retained one, and the owner dies at its own last read.
#[test]
fn owner_read_after_the_call_is_retained_for_it() {
    let mut func = function("live_owner_retained", &[]);
    let value = owned(&mut func);
    body(
        &mut func,
        vec![
            produce(value),
            transfer(vec![value]),
            marker(),
            borrow(vec![value]),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// An op that borrows a root and adopts it too, like a `super()` method call
/// whose class is also its argument, keeps the root's own reference through the
/// op: the adoption takes a retained one. The dead receiver moves.
#[test]
fn owner_borrowed_by_its_adopting_call_is_retained() {
    let mut func = function("borrowed_and_adopted", &[]);
    let value = owned(&mut func);
    let receiver = owned(&mut func);
    body(
        &mut func,
        vec![
            produce(value),
            produce(receiver),
            source_call(
                "call_super_method_ic",
                None,
                vec![value, receiver, value],
                vec![],
                &[BORROWED, TRANSFERRED, TRANSFERRED],
            ),
            marker(),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(1), Event::Freed(0), Event::Marker]
    );
}

/// An ordinary source call (CALL) adopts its temporary callable, so the
/// invocation releases it at the call, as it releases a bound method before
/// the callee runs. An expanded call (CALL_FUNCTION_EX) borrows its callable
/// through the invocation, and the caller releases it after the call. The
/// builder moves either way.
#[test]
fn ordinary_call_adopts_its_callable_and_expanded_call_borrows_it() {
    for (form, callable_custody, expected) in [
        (
            None,
            TRANSFERRED,
            [Event::Freed(0), Event::Freed(1), Event::Marker],
        ),
        (
            Some("expanded"),
            BORROWED,
            [Event::Freed(1), Event::Freed(0), Event::Marker],
        ),
    ] {
        let mut func = function("calls_temporary_callable", &[]);
        let callable = owned(&mut func);
        let builder = owned(&mut func);
        let mut new_builder = original_copy("callargs_new", vec![builder]);
        if let Some(form) = form {
            new_builder
                .attrs
                .insert("s_value".into(), AttrValue::Str(form.into()));
        }
        body(
            &mut func,
            vec![
                produce(callable),
                new_builder,
                source_call(
                    "call_bind",
                    None,
                    vec![callable, builder],
                    vec![],
                    &[callable_custody, TRANSFERRED],
                ),
                marker(),
            ],
            done(vec![]),
        );
        insert(&mut func);
        assert_eq!(trace(&func, 0, &[]), expected, "{form:?}");
    }
}

/// A borrowed parameter stays the caller's: a call that adopts it receives a
/// retained reference, and the parameter still holds the caller's at exit.
#[test]
fn borrowed_parameter_is_retained_for_an_adopting_call() {
    let mut func = function("borrowed_parameter_passed_on", &[BORROWED]);
    let param = parameter(&func, 0);
    body(
        &mut func,
        vec![transfer(vec![param]), marker()],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker]);
}

/// A named local keeps its object until its frame releases it, as CPython's
/// frame slot does: the call adopts a retained reference, and the object dies
/// at the return, after the statement that follows the call.
#[test]
fn named_local_outlives_the_call_that_adopts_it() {
    let mut func = function("named_local_passed_on", &[]);
    let value = owned(&mut func);
    let mut ops = Vec::from(bind_local(value));
    ops.extend([transfer(vec![value]), marker()]);
    body(&mut func, ops, done(vec![]));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// A raw carrier holds no reference: an adopting call takes none from it, and
/// the pass places no reference operation on it.
#[test]
fn raw_operand_carries_no_reference_into_the_call() {
    let mut func = function("raw_operand", &[]);
    let raw = flag(&mut func);
    body(
        &mut func,
        vec![
            TirOp {
                attrs: AttrDict::from([("value".into(), molt_ir::tir::ops::AttrValue::Bool(true))]),
                ..op(OpCode::ConstBool, vec![], vec![raw])
            },
            transfer(vec![raw]),
        ],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), Vec::<Event>::new());
}

/// A call adopts its operands whether it returns or raises: neither the normal
/// continuation nor the observation's landing releases the owner it moved.
#[test]
fn moved_owner_is_released_by_neither_continuation() {
    let mut func = function("moved_owner_raises", &[]);
    let value = owned(&mut func);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 41);
    body(
        &mut func,
        vec![produce(value), transfer(vec![value]), observe(41), marker()],
        done(vec![]),
    );
    func.blocks
        .insert(handler, block(handler, vec![marker()], done(vec![])));
    insert(&mut func);
    for raises in [false, true] {
        assert_eq!(trace(&func, 0, &[raises]), [Event::Freed(0), Event::Marker]);
    }
}

/// An owner that the handler still reads cannot move into a call that may
/// raise into it: the call adopts a retained reference, and the owner survives
/// for the handler.
#[test]
fn owner_the_handler_reads_is_retained_for_the_call() {
    let mut func = function("handler_reads_owner", &[]);
    let value = owned(&mut func);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 42);
    body(
        &mut func,
        vec![produce(value), transfer(vec![value]), observe(42), marker()],
        done(vec![]),
    );
    func.blocks.insert(
        handler,
        block(handler, vec![borrow(vec![value]), marker()], done(vec![])),
    );
    insert(&mut func);
    for raises in [false, true] {
        assert_eq!(trace(&func, 0, &[raises]), [Event::Freed(0), Event::Marker]);
    }
}

/// An owner moved into a call on one arm is released on the other arm, once,
/// and never again at the join.
#[test]
fn owner_moved_on_one_arm_is_released_on_the_other() {
    let mut func = function("moved_on_one_arm", &[]);
    let value = owned(&mut func);
    let cond = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let cond_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (then_block, else_block, join) =
        (func.fresh_block(), func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![
            produce(value),
            op(OpCode::Copy, vec![cond_input], vec![cond]),
        ],
        choose(cond, then_block, else_block),
    );
    func.blocks.insert(
        then_block,
        block(then_block, vec![transfer(vec![value])], jump(join)),
    );
    func.blocks
        .insert(else_block, block(else_block, vec![], jump(join)));
    func.blocks
        .insert(join, block(join, vec![marker()], done(vec![])));
    insert(&mut func);
    for taken in [true, false] {
        assert_eq!(trace(&func, 0, &[taken]), [Event::Freed(0), Event::Marker]);
    }
}

/// An owner that the join still reads is retained for a call on one arm, and
/// released once at its last read on every path.
#[test]
fn owner_read_after_the_join_is_retained_on_its_arm() {
    let mut func = function("retained_on_one_arm", &[]);
    let value = owned(&mut func);
    let cond = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let cond_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (then_block, else_block, join) =
        (func.fresh_block(), func.fresh_block(), func.fresh_block());
    body(
        &mut func,
        vec![
            produce(value),
            op(OpCode::Copy, vec![cond_input], vec![cond]),
        ],
        choose(cond, then_block, else_block),
    );
    func.blocks.insert(
        then_block,
        block(then_block, vec![transfer(vec![value])], jump(join)),
    );
    func.blocks
        .insert(else_block, block(else_block, vec![], jump(join)));
    func.blocks.insert(
        join,
        block(join, vec![borrow(vec![value]), marker()], done(vec![])),
    );
    insert(&mut func);
    for taken in [true, false] {
        assert_eq!(trace(&func, 0, &[taken]), [Event::Freed(0), Event::Marker]);
    }
}

/// A loop-invariant owner passed to an adopting call in the body is retained on
/// every iteration and released once, after the loop.
#[test]
fn loop_invariant_owner_is_retained_on_every_iteration() {
    let mut func = function("loop_invariant_owner", &[]);
    let value = owned(&mut func);
    let more = flag(&mut func);
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let more_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let (header, loop_body, exit) = (func.fresh_block(), func.fresh_block(), func.fresh_block());
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    body(&mut func, vec![produce(value)], jump(header));
    func.blocks.insert(
        header,
        block(
            header,
            vec![op(OpCode::Copy, vec![more_input], vec![more])],
            choose(more, loop_body, exit),
        ),
    );
    func.blocks.insert(
        loop_body,
        block(loop_body, vec![transfer(vec![value])], jump(header)),
    );
    func.blocks
        .insert(exit, block(exit, vec![marker()], done(vec![])));
    insert(&mut func);
    for iterations in 0..3 {
        let mut choices = vec![true; iterations];
        choices.push(false);
        assert_eq!(trace(&func, 0, &choices), [Event::Freed(0), Event::Marker]);
    }
}

/// A transferred parameter belongs to the activation like a local: its frame
/// releases it at exit, after the body's last statement rather than at its last
/// read, and never leaves it to the caller.
#[test]
fn transferred_parameter_is_released_at_frame_exit() {
    let mut func = function("owns_parameter", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    body(&mut func, vec![borrow(vec![param]), marker()], done(vec![]));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// A transferred parameter passed to an adopting call stays bound in its
/// frame: the call adopts a retained reference, and the frame releases its own
/// at exit.
#[test]
fn transferred_parameter_passed_on_stays_bound_until_frame_exit() {
    let mut func = function("passes_parameter_on", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    body(
        &mut func,
        vec![transfer(vec![param]), marker()],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker, Event::Freed(0)]);
}

/// A raising path out of the activation releases its transferred parameter
/// exactly once, as the normal return does.
#[test]
fn transferred_parameter_is_released_on_the_exceptional_exit() {
    let mut func = function("parameter_on_exception", &[TRANSFERRED]);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 43);
    body(
        &mut func,
        vec![borrow(vec![]), observe(43), marker()],
        done(vec![]),
    );
    func.blocks
        .insert(handler, block(handler, vec![], done(vec![])));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[false]), [Event::Marker, Event::Freed(0)]);
    assert_eq!(trace(&func, 0, &[true]), [Event::Freed(0)]);
}

/// Returning a transferred parameter moves the activation's reference to the
/// caller, with no publication retain.
#[test]
fn returned_transferred_parameter_moves_its_reference() {
    let mut func = function("returns_parameter", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    body(&mut func, vec![marker()], done(vec![param]));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Marker]);
}

/// Rebinding a transferred parameter releases the caller's argument at the
/// store, as CPython's `STORE_FAST` does, before the statements that follow;
/// the replacement lives to the frame's exit.
#[test]
fn rebinding_a_transferred_parameter_releases_the_argument_at_the_store() {
    let mut func = function("rebinds_parameter", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let replacement = owned(&mut func);
    let mut ops = Vec::from(bind_local(replacement));
    ops.extend([op(OpCode::DelBoundary, vec![param], vec![]), marker()]);
    body(&mut func, ops, done(vec![]));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(0), Event::Marker, Event::Freed(1)]
    );
}

/// The frontend's capture of a read that a later store in the same expression
/// rebinds: an owned alias, CPython's `LOAD_FAST` stack reference.
fn capture(read: ValueId, result: ValueId) -> TirOp {
    original_copy_with_operands("binding_alias", vec![read], vec![result])
}

/// `f(x, (x := g()))` with `x` a transferred parameter. The store releases the
/// argument's frame slot; the captured read moves into the call and ends at the
/// callee's exit, as CPython's stack reference does; the new binding stays in
/// the frame until it exits. Without the capture the call would retain a
/// released object.
#[test]
fn captured_read_moves_into_the_call_that_its_rebinding_precedes() {
    let mut func = function("rebind_in_call", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let read = owned(&mut func);
    let replacement = owned(&mut func);
    let mut ops = vec![capture(param, read)];
    ops.extend(bind_local(replacement));
    ops.extend([
        op(OpCode::DelBoundary, vec![param], vec![]),
        transfer(vec![read, replacement]),
        marker(),
    ]);
    body(&mut func, ops, done(vec![]));
    insert(&mut func);
    assert_eq!(
        trace(&func, 0, &[]),
        [Event::Freed(0), Event::Marker, Event::Freed(1)]
    );
}

/// `(x, (x := B()))` and `x + (x := B())`: the captured read outlives the store
/// that releases the argument, and ends after its consumer; the new binding
/// stays in the frame until it exits.
#[test]
fn captured_read_outlives_its_rebinding_until_its_consumer() {
    for consumer in [OpCode::BuildTuple, OpCode::Add] {
        let mut func = function("rebind_in_expression", &[TRANSFERRED]);
        let param = parameter(&func, 0);
        let read = owned(&mut func);
        let replacement = owned(&mut func);
        let combined = owned(&mut func);
        let mut ops = vec![capture(param, read)];
        ops.extend(bind_local(replacement));
        ops.extend([
            op(OpCode::DelBoundary, vec![param], vec![]),
            op(consumer, vec![read, replacement], vec![combined]),
            marker(),
        ]);
        body(&mut func, ops, done(vec![]));
        insert(&mut func);
        assert_eq!(
            trace(&func, 0, &[]),
            [
                Event::Freed(0),
                Event::Freed(2),
                Event::Marker,
                Event::Freed(1)
            ]
        );
    }
}

/// `return x` from a frame whose exit releases another binding first. That
/// release may run a finalizer that observes the frame or rebinds `x`, so the
/// return expression is captured before the teardown starts, as CPython's
/// `LOAD_FAST` precedes its frame clear. The exit then releases every binding,
/// the returned one included, and the return moves the capture's reference to
/// the caller. A retain for the return would leak, and a release of the
/// capture would free the result.
#[test]
fn return_captured_before_frame_teardown_survives_it() {
    let mut func = function("returns_captured_binding", &[TRANSFERRED]);
    let param = parameter(&func, 0);
    let local = owned(&mut func);
    let returned = owned(&mut func);
    let mut ops = Vec::from(bind_local(local));
    ops.extend([
        capture(param, returned),
        op(OpCode::DelBoundary, vec![local], vec![]),
        op(OpCode::DelBoundary, vec![param], vec![]),
        marker(),
    ]);
    body(&mut func, ops, done(vec![returned]));
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Freed(1), Event::Marker]);
}

/// A CallArgs builder that `call_bind` frees is adopted like a transferred
/// argument: it moves into the call, which releases it, and needs no release
/// after it.
#[test]
fn callargs_builder_moves_into_call_bind() {
    let mut func = function("builder_moves", &[BORROWED]);
    let callee = parameter(&func, 0);
    let builder = owned(&mut func);
    let bind = call_bind(callee, builder, vec![]);
    body(
        &mut func,
        vec![original_copy("callargs_new", vec![builder]), bind, marker()],
        done(vec![]),
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), [Event::Freed(1), Event::Marker]);
}

// ── Inlined activations ─────────────────────────────────────────────────────

/// Inlines the one direct call in `caller` to `callee` through the module
/// inliner, whose re-optimization of the merged caller runs too, then places
/// the caller's RC.
fn inline(caller: TirFunction, callee: TirFunction) -> TirFunction {
    // The module inliner consumes lifted bodies. Lift through the public
    // producer so call-return observations are authored before SSA, as in
    // production; do not fabricate async-work metadata in this fixture.
    let document = inline_fixture_document(&caller, &callee);
    let caller = lift_inline_fixture(&document.functions[0]);
    let callee = lift_inline_fixture(&document.functions[1]);
    let mut module = TirModule {
        name: "inline_custody".into(),
        functions: vec![caller, callee],
    };
    let graph = CallGraph::build(&module);
    let summaries = ModuleSummaries::compute(&module, &graph);
    let stats = run_inliner(
        &mut module,
        &graph,
        &summaries,
        &TargetInfo::native_release_fast(),
        &HashSet::new(),
    );
    let mut merged = module.functions.swap_remove(0);
    assert_eq!(
        stats.sites_inlined, 1,
        "{}: the call must inline",
        merged.name
    );
    insert(&mut merged);
    merged
}

fn inline_fixture_document(caller: &TirFunction, callee: &TirFunction) -> molt_ir::SimpleIR {
    let functions = [caller, callee]
        .into_iter()
        .map(|func| {
            molt_passes::tir::verify::verify_function(func)
                .expect("the authored inliner fixture must be valid before transport");
            molt_ir::FunctionIR {
                return_abi: func.return_abi,
                name: func.name.clone(),
                params: func.param_names.clone(),
                param_types: None,
                ops: molt_passes::tir::lower_to_simple::lower_to_simple_ir(func),
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                // The verified TIR projection already owns the canonical
                // absence of all-borrowed custody. Preserve that absence on
                // the wire; do not expand it into a second vector encoding.
                parameter_custody: if func
                    .attrs
                    .contains_key(molt_ir::tir::function::PARAMETER_CUSTODY_ATTR)
                {
                    (0..func.param_types.len())
                        .map(|position| func.parameter_custody(position))
                        .collect()
                } else {
                    Vec::new()
                },
                execution_context: func.execution_context,
            }
        })
        .collect();
    let document = molt_ir::SimpleIR {
        functions,
        profile: None,
    };
    molt_ir::validate_simple_ir(&document).expect(
        "the whole inline fixture must satisfy production schema and call custody admission",
    );
    let report = molt_ir::verify_simple_ir(&document);
    assert!(
        report.is_ok(),
        "the whole inline fixture must satisfy production reference admission: {:?}",
        report.errors
    );
    document
}

fn lift_inline_fixture(simple: &molt_ir::FunctionIR) -> TirFunction {
    let lifted = molt_passes::tir::lower_from_simple::lower_to_tir_for_target(
        simple,
        &TargetInfo::native_release_fast(),
    );
    molt_passes::tir::verify::verify_function(&lifted)
        .expect("the inliner fixture must enter through valid canonical SSA");
    for op in lifted
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::Is)
    {
        assert_eq!(
            lifted.value_types.get(&op.results[0]),
            Some(&TirType::Bool),
            "lifted Is must be Bool so the RC oracle never allocates it an object identity"
        );
    }
    lifted
}

/// Opaque finalization/arithmetic can recursively re-enter a generic callee.
/// Keep that refusal explicit and prove both source activations with the same
/// ordinary-call RC oracle used by the non-inline custody cases above.
fn refused_opaque_inline(caller: TirFunction, callee: TirFunction) -> (TirFunction, TirFunction) {
    use molt_passes::tir::call_facts::{InlineEligibility, InlineWhyNot};
    use molt_passes::tir::passes::inliner::classify_inline_eligibility;
    // Even refusal fixtures must satisfy the same document custody/reference
    // boundary; their original activations remain the independent RC oracle.
    inline_fixture_document(&caller, &callee);
    let mut module = TirModule {
        name: "opaque_inline_custody".into(),
        functions: vec![caller, callee],
    };
    let before: Vec<_> = module
        .functions
        .iter()
        .map(molt_passes::tir::printer::print_function)
        .collect();
    let graph = CallGraph::build(&module);
    let summaries = ModuleSummaries::compute(&module, &graph);
    let target = TargetInfo::native_release_fast();
    assert_eq!(
        classify_inline_eligibility(&module.functions[1], &graph, &summaries, &target),
        InlineEligibility::WhyNot(InlineWhyNot::Recursive)
    );
    let stats = run_inliner(&mut module, &graph, &summaries, &target, &HashSet::new());
    assert_eq!(stats.sites_inlined, 0);
    assert_eq!(stats.functions_changed, 0);
    assert_eq!(
        module
            .functions
            .iter()
            .map(molt_passes::tir::printer::print_function)
            .collect::<Vec<_>>(),
        before,
        "refusal must preserve both source activations"
    );
    let mut callee = module.functions.pop().unwrap();
    let mut caller = module.functions.pop().unwrap();
    insert(&mut callee);
    insert(&mut caller);
    (caller, callee)
}

/// A direct call of `callee` whose operands take its parameters' custody.
fn call(callee: &TirFunction, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let custody: Vec<ParameterCustody> = (0..callee.param_types.len())
        .map(|position| callee.parameter_custody(position))
        .collect();
    source_call(
        "call_internal",
        Some(&callee.name),
        operands,
        results,
        &custody,
    )
}

/// An observable statement that reads `values`.
fn read(values: Vec<ValueId>) -> TirOp {
    op(OpCode::WarnStderr, values, vec![])
}

/// A callee that owns its parameters still owns them inlined. A temporary
/// argument outlives the body's last read of it and dies at the callee's frame
/// clear, before the caller's next statement. Passed twice, it owes two
/// references, and both end there.
#[test]
fn inlined_owned_parameters_die_at_the_callee_frame_clear() {
    for arity in [1, 2] {
        let mut callee = function("owns_parameters", &vec![TRANSFERRED; arity]);
        let parameters: Vec<ValueId> = (0..arity)
            .map(|position| parameter(&callee, position))
            .collect();
        let mut reads: Vec<_> = parameters
            .into_iter()
            .map(|value| read(vec![value]))
            .collect();
        reads.push(marker());
        body(&mut callee, reads, done(vec![]));
        let mut caller = function("passes_a_temporary", &[]);
        let value = owned(&mut caller);
        body(
            &mut caller,
            vec![
                produce(value),
                call(&callee, vec![value; arity], vec![]),
                marker(),
            ],
            done(vec![]),
        );
        let merged = inline(caller, callee);
        assert_eq!(
            trace(&merged, 0, &[]),
            std::iter::repeat_n(Event::Marker, arity + 1)
                .chain([Event::Freed(0), Event::Marker])
                .collect::<Vec<_>>(),
            "{arity} independently read parameters"
        );
    }
}

/// An argument the caller still reads outlives the inlined frame clear: the
/// callee's binding holds a reference of its own, and the object dies at the
/// caller's own last read.
#[test]
fn inlined_owned_parameter_leaves_a_live_argument_to_the_caller() {
    let mut callee = function("owns_parameter", &[TRANSFERRED]);
    let param = parameter(&callee, 0);
    body(&mut callee, vec![read(vec![param])], done(vec![]));
    let mut caller = function("reads_after_the_call", &[]);
    let value = owned(&mut caller);
    body(
        &mut caller,
        vec![
            produce(value),
            call(&callee, vec![value], vec![]),
            marker(),
            read(vec![value]),
        ],
        done(vec![]),
    );
    let merged = inline(caller, callee);
    assert_eq!(
        trace(&merged, 0, &[]),
        [Event::Marker, Event::Marker, Event::Marker, Event::Freed(0)]
    );
}

/// Every exit of the inlined callee clears its frame: the early return and the
/// fall-through both release the owned binding before the caller resumes. The
/// borrowed argument, the caller's own parameter, binds directly and stays the
/// caller's.
#[test]
fn every_inlined_exit_clears_the_frame() {
    let mut callee = function("returns_early", &[TRANSFERRED, BORROWED]);
    let (param, other) = (parameter(&callee, 0), parameter(&callee, 1));
    let same = flag(&mut callee);
    let (early, late) = (callee.fresh_block(), callee.fresh_block());
    body(
        &mut callee,
        vec![op(OpCode::Is, vec![param, other], vec![same])],
        choose(same, early, late),
    );
    callee
        .blocks
        .insert(early, block(early, vec![marker()], done(vec![])));
    callee.blocks.insert(
        late,
        block(late, vec![read(vec![param]), marker()], done(vec![])),
    );
    let mut caller = function("calls_with_an_early_return", &[BORROWED]);
    let own = parameter(&caller, 0);
    let value = owned(&mut caller);
    body(
        &mut caller,
        vec![
            produce(value),
            call(&callee, vec![value, own], vec![]),
            marker(),
        ],
        done(vec![]),
    );
    let merged = inline(caller, callee);
    // Object 0 is the caller's parameter, object 1 its temporary.
    assert_eq!(
        trace(&merged, 0, &[false, true]),
        [Event::Marker, Event::Freed(1), Event::Marker]
    );
    assert_eq!(
        trace(&merged, 0, &[false, false]),
        [Event::Marker, Event::Marker, Event::Freed(1), Event::Marker]
    );
}

/// A `del` of an owned parameter inside the inlined body releases the binding
/// at the statement, as in the callee, and the frame clear releases it no
/// second time.
#[test]
fn inlined_del_of_an_owned_parameter_releases_it_there() {
    let mut callee = function("deletes_parameter", &[TRANSFERRED]);
    let param = parameter(&callee, 0);
    body(
        &mut callee,
        vec![
            read(vec![param]),
            op(OpCode::DelBoundary, vec![param], vec![]),
            marker(),
        ],
        done(vec![]),
    );
    let mut caller = function("passes_to_del", &[]);
    let value = owned(&mut caller);
    body(
        &mut caller,
        vec![produce(value), call(&callee, vec![value], vec![]), marker()],
        done(vec![]),
    );
    let (caller, callee) = refused_opaque_inline(caller, callee);
    assert_eq!(
        trace(&callee, 0, &[]),
        [Event::Marker, Event::Freed(0), Event::Marker],
        "the generic callee preserves its exact binding/finalizer boundary"
    );
    assert_eq!(
        trace(&caller, 0, &[]),
        [Event::Freed(0), Event::Marker],
        "the retained source call preserves the caller's reference obligations"
    );
}

/// A raise inside the inlined body clears the callee frame once on its way to
/// the caller's handler, as the returning path does.
#[test]
fn inlined_raise_clears_the_frame_before_the_caller_handler() {
    let mut callee = function("raises", &[TRANSFERRED]);
    callee.has_exception_handling = true;
    let param = parameter(&callee, 0);
    let exit = callee.fresh_block();
    callee.label_id_map.insert(exit.0, 44);
    body(
        &mut callee,
        vec![read(vec![param]), observe(44), marker()],
        done(vec![]),
    );
    callee
        .blocks
        .insert(exit, block(exit, vec![], done(vec![])));
    let mut caller = function("handles", &[]);
    caller.has_exception_handling = true;
    let value = owned(&mut caller);
    let handler = caller.fresh_block();
    caller.label_id_map.insert(handler.0, 45);
    body(
        &mut caller,
        vec![
            produce(value),
            call(&callee, vec![value], vec![]),
            observe(45),
            marker(),
        ],
        done(vec![]),
    );
    caller
        .blocks
        .insert(handler, block(handler, vec![marker()], done(vec![])));
    let merged = inline(caller, callee);
    // The callee raises, and the caller's observation delivers the exception.
    assert_eq!(
        trace(&merged, 0, &[false, true, true]),
        [Event::Marker, Event::Freed(0), Event::Marker]
    );
    assert_eq!(
        trace(&merged, 0, &[false, false, false]),
        [Event::Marker, Event::Marker, Event::Freed(0), Event::Marker]
    );
}

/// A borrowed helper's parameter stays the caller's argument inlined: a `del`
/// in the helper releases nothing, and a temporary argument dies after the
/// whole helper body, where the caller releases a borrowed argument after the
/// call.
#[test]
fn inlined_borrowed_parameter_lives_to_the_end_of_the_helper() {
    let mut callee = function("borrows_parameter", &[BORROWED]);
    let param = parameter(&callee, 0);
    body(
        &mut callee,
        vec![
            read(vec![param]),
            op(OpCode::DelBoundary, vec![param], vec![]),
            marker(),
        ],
        done(vec![]),
    );
    let mut caller = function("passes_a_temporary_to_a_helper", &[]);
    let value = owned(&mut caller);
    body(
        &mut caller,
        vec![produce(value), call(&callee, vec![value], vec![]), marker()],
        done(vec![]),
    );
    let (caller, callee) = refused_opaque_inline(caller, callee);
    assert_eq!(
        trace(&callee, 0, &[]),
        [Event::Marker, Event::Marker],
        "the generic callee preserves its exact binding/finalizer boundary"
    );
    assert_eq!(
        trace(&caller, 0, &[]),
        [Event::Freed(0), Event::Marker],
        "the retained source call preserves the caller's reference obligations"
    );
}

/// A callee that returns its parameter, owned or borrowed, hands the caller an
/// independent reference taken before its frame clear: the binding ends at the
/// callee's exit, and the result at the caller's last read of it.
#[test]
fn inlined_result_naming_a_parameter_outlives_the_frame_clear() {
    for custody in [TRANSFERRED, BORROWED] {
        let mut callee = function("returns_parameter", &[custody]);
        let param = parameter(&callee, 0);
        body(&mut callee, vec![read(vec![param])], done(vec![param]));
        let mut caller = function("reads_the_result", &[]);
        let value = owned(&mut caller);
        let result = owned(&mut caller);
        body(
            &mut caller,
            vec![
                produce(value),
                call(&callee, vec![value], vec![result]),
                read(vec![result]),
                marker(),
            ],
            done(vec![]),
        );
        let merged = inline(caller, callee);
        assert_eq!(
            trace(&merged, 0, &[]),
            [Event::Marker, Event::Marker, Event::Freed(0), Event::Marker],
            "{custody:?}"
        );
    }
}

/// The re-optimization of the merged caller numbers a helper's local, `s + s`
/// of the argument it borrows, onto the caller's equal local. The helper's
/// `del` ends its own binding only: the caller's value keeps its reference to
/// its last read.
#[test]
fn inlined_local_equal_to_a_caller_local_keeps_its_own_reference() {
    let mut callee = function("deletes_its_concat", &[BORROWED]);
    let param = parameter(&callee, 0);
    let local = owned(&mut callee);
    body(
        &mut callee,
        vec![
            op(OpCode::Add, vec![param, param], vec![local]),
            read(vec![local]),
            op(OpCode::DelBoundary, vec![local], vec![]),
            marker(),
        ],
        done(vec![]),
    );
    let mut caller = function("keeps_its_concat", &[]);
    let text = owned(&mut caller);
    let kept = owned(&mut caller);
    body(
        &mut caller,
        vec![
            const_str(text),
            op(OpCode::Add, vec![text, text], vec![kept]),
            call(&callee, vec![text], vec![]),
            read(vec![text]),
            read(vec![kept]),
        ],
        done(vec![]),
    );
    let (caller, callee) = refused_opaque_inline(caller, callee);
    assert_eq!(
        trace(&callee, 0, &[]),
        [Event::Marker, Event::Freed(1), Event::Marker],
        "the generic callee preserves its exact binding/finalizer boundary"
    );
    assert_eq!(
        trace(&caller, 0, &[]),
        [
            Event::Marker,
            Event::Freed(0),
            Event::Marker,
            Event::Freed(1)
        ],
        "the retained source call preserves the caller's reference obligations"
    );
}
