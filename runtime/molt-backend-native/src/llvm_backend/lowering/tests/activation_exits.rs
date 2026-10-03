//! LLVM consumers of the activation exits that terminal drop insertion makes
//! explicit. The poll frame parameter carries every scheduler call, the pending
//! test is the exact sentinel word, and each Return transfers the one poll
//! owner that TIR placed.

use super::*;

const RESUME_STATE: i64 = 7;
const RUNNING_STATE: i64 = 9;

struct NormalizedWait {
    func: TirFunction,
    pending: BlockId,
    ready: BlockId,
}

fn tir_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>, attrs: AttrDict) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

fn state_attr(state: i64) -> AttrDict {
    AttrDict::from([("value".into(), AttrValue::Int(state))])
}

/// The wait `drop_insertion::activation` exposes, over the poll frame
/// (parameter 0) and a borrowed future (parameter 1): save state 7, poll, test
/// the poll word, then a pending exit that registers the wait and returns the
/// poll word and a ready exit that saves the running state 9 and returns it.
/// State 7 is the dispatch case that resumes the poll block; no case resumes
/// state 9. With `poll_call` the poll is the normalized `molt_future_poll`
/// Call; otherwise the poll word is borrowed parameter 2.
fn normalized_wait(name: &str, poll_call: bool) -> NormalizedWait {
    let params = vec![TirType::DynBox; if poll_call { 2 } else { 3 }];
    let mut func = TirFunction::new(
        name.into(),
        params,
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.param_names[0] = "self".into();
    let future = ValueId(1);
    let polled = if poll_call {
        func.fresh_value()
    } else {
        ValueId(2)
    };
    func.value_types.insert(polled, TirType::DynBox);
    let waiting = func.fresh_value();
    func.value_types.insert(waiting, TirType::Bool);
    let poll = func.fresh_block();
    let pending = func.fresh_block();
    let ready = func.fresh_block();
    let mut poll_ops = vec![tir_op(
        OpCode::StateSet,
        vec![],
        vec![],
        state_attr(RESUME_STATE),
    )];
    if poll_call {
        poll_ops.push(tir_op(
            OpCode::Call,
            vec![future],
            vec![polled],
            AttrDict::from([("s_value".into(), AttrValue::Str("molt_future_poll".into()))]),
        ));
    }
    poll_ops.push(tir_op(
        OpCode::IsPending,
        vec![polled],
        vec![waiting],
        AttrDict::new(),
    ));
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(RESUME_STATE, poll, vec![])],
        default: poll,
        default_args: vec![],
    };
    func.blocks.insert(
        poll,
        TirBlock {
            id: poll,
            args: vec![],
            ops: poll_ops,
            terminator: Terminator::CondBranch {
                cond: waiting,
                then_block: pending,
                then_args: vec![],
                else_block: ready,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        pending,
        TirBlock {
            id: pending,
            args: vec![],
            ops: vec![tir_op(
                OpCode::TaskWait,
                vec![future],
                vec![],
                AttrDict::new(),
            )],
            terminator: Terminator::Return {
                values: vec![polled],
            },
        },
    );
    func.blocks.insert(
        ready,
        TirBlock {
            id: ready,
            args: vec![],
            ops: vec![tir_op(
                OpCode::StateSet,
                vec![],
                vec![],
                state_attr(RUNNING_STATE),
            )],
            terminator: Terminator::Return {
                values: vec![polled],
            },
        },
    );
    NormalizedWait {
        func,
        pending,
        ready,
    }
}

#[test]
fn activation_ops_save_state_test_the_exact_sentinel_and_register_the_frame() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let wait = normalized_wait("activation_ops", false);
    let llvm_fn = try_lower_tir_to_llvm(&wait.func, &backend)
        .unwrap_or_else(|err| panic!("activation exits must lower: {err}"));
    backend
        .module
        .verify()
        .expect("activation exits must verify");
    let ir = llvm_fn.print_to_string().to_string();
    let pending = molt_codegen_abi::pending_bits();

    // StateSet saves each constant state through the frame parameter,
    // including a running state that no dispatch case resumes.
    for state in [RESUME_STATE, RUNNING_STATE] {
        let save = format!("call void @molt_obj_set_state(i64 %0, i64 {state})");
        assert_eq!(ir.matches(save.as_str()).count(), 1, "{ir}");
    }
    // IsPending compares the poll word itself with the sentinel, and the
    // branch consumes that bit: no truthiness, no callback.
    assert!(
        ir.contains(&format!("%is_pending = icmp eq i64 %2, {pending}")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!(
            "br i1 %is_pending, label %bb{}, label %bb{}",
            wait.pending.0, wait.ready.0
        )),
        "{ir}"
    );
    assert!(!ir.contains("@molt_is_truthy("), "{ir}");
    // TaskWait registers the raw frame and the future's object address.
    assert!(
        ir.contains("%task_wait_frame = inttoptr i64 %0 to ptr"),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("and i64 %1, {}", nanbox::POINTER_MASK)),
        "{ir}"
    );
    assert!(
        ir.contains("@molt_sleep_register(ptr %task_wait_frame, ptr %task_wait_future)"),
        "{ir}"
    );
    // Both exits transfer the one poll owner: no retain and no release.
    assert!(!ir.contains("@molt_inc_ref_obj("), "{ir}");
    assert!(!ir.contains("@molt_dec_ref_obj("), "{ir}");
    assert_eq!(ir.matches("ret i64 %2\n").count(), 2, "{ir}");
}

/// A first-class TIR `Call` names its static target in `s_value`. With a
/// canonical runtime row it lowers through that row; without one it fails
/// closed. `molt_chan_recv/1` is an existing poll row.
#[test]
fn first_class_calls_use_only_their_canonical_runtime_row() {
    for (symbol, classified) in [("molt_chan_recv", true), ("molt_unclassified_poll", false)] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.runtime_callable_symbols.insert(symbol.into());
        let mut func = TirFunction::new(
            format!("first_class_{symbol}"),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(tir_op(
            OpCode::Call,
            vec![ValueId(0)],
            vec![result],
            AttrDict::from([("s_value".into(), AttrValue::Str(symbol.into()))]),
        ));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let lowered = try_lower_tir_to_llvm(&func, &backend);
        if classified {
            let ir = lowered
                .unwrap_or_else(|err| panic!("{symbol}: {err}"))
                .print_to_string()
                .to_string();
            assert!(
                ir.contains(&format!("%{symbol} = call i64 @{symbol}(i64 %0)")),
                "{ir}"
            );
            // The owned poll word is the returned value itself.
            assert!(ir.contains(&format!("ret i64 %{symbol}\n")), "{ir}");
            assert!(!ir.contains("@molt_inc_ref_obj("), "{ir}");
            assert!(!ir.contains("@molt_dec_ref_obj("), "{ir}");
        } else {
            let err = lowered.expect_err("an unclassified first-class call must fail closed");
            assert_lowering_error_contains(&err, "has no exact native linkage ABI");
        }
    }
}

/// The normalized wait's poll is a first-class `Call` to `molt_future_poll`.
/// Red until the generated boxed-ABI table carries its `molt_future_poll/1`
/// poll row (HANDOFF blocker B1): no other authority may type that call.
#[test]
fn normalized_wait_polls_through_the_canonical_poll_row() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_future_poll".into());
    let wait = normalized_wait("activation_poll", true);
    let llvm_fn = try_lower_tir_to_llvm(&wait.func, &backend)
        .unwrap_or_else(|err| panic!("the normalized wait must lower: {err}"));
    backend
        .module
        .verify()
        .expect("the normalized wait must verify");
    let ir = llvm_fn.print_to_string().to_string();
    let pending = molt_codegen_abi::pending_bits();
    // The poll borrows the future word; its owned result is the word tested
    // and returned, with no retain or release on either exit.
    assert_eq!(
        ir.matches("%molt_future_poll = call i64 @molt_future_poll(i64 %1)")
            .count(),
        1,
        "{ir}"
    );
    assert!(
        ir.contains(&format!(
            "%is_pending = icmp eq i64 %molt_future_poll, {pending}"
        )),
        "{ir}"
    );
    assert!(!ir.contains("@molt_inc_ref_obj("), "{ir}");
    assert!(!ir.contains("@molt_dec_ref_obj("), "{ir}");
    assert_eq!(ir.matches("ret i64 %molt_future_poll\n").count(), 2, "{ir}");
}

#[test]
#[should_panic(expected = "must expose it as explicit activation exits")]
fn drop_inserted_lowering_rejects_a_hidden_suspension() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "hidden_suspension".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.param_names[0] = "self".into();
    func.attrs.insert(
        crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.into(),
        AttrValue::Bool(true),
    );
    let first = func.fresh_block();
    let resume = func.fresh_block();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(RESUME_STATE, resume, vec![])],
        default: first,
        default_args: vec![],
    };
    func.blocks.insert(
        first,
        TirBlock {
            id: first,
            args: vec![],
            ops: vec![tir_op(
                OpCode::StateYield,
                vec![ValueId(1)],
                vec![],
                state_attr(RESUME_STATE),
            )],
            terminator: Terminator::Unreachable,
        },
    );
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(1)],
            },
        },
    );
    let _ = try_lower_tir_to_llvm(&func, &backend);
}
