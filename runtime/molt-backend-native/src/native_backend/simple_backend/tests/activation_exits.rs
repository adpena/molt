//! Native consumers of the activation exits that terminal drop insertion makes
//! explicit. The poll frame parameter carries every scheduler call, the pending
//! test is the exact sentinel word, and a Return transfers the one poll owner
//! that TIR placed.

use super::*;
use crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR;

const SELF: usize = 0;
const FUTURE: usize = 1;
const POLLED: usize = 2;
const RESUME_STATE: i64 = 7;
const RESUME_LABEL: i64 = 42;
const WAIT_LABEL: i64 = 8;
const RUNNING_STATE: i64 = 9;

fn simple_op(kind: &str, args: Option<&[&str]>, out: Option<&str>, value: Option<i64>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: args.map(|args| args.iter().map(|arg| arg.to_string()).collect()),
        out: out.map(str::to_string),
        value,
        ..OpIR::default()
    }
}

/// A drop-inserted poll function over its frame, a borrowed future and a
/// borrowed poll result. State 7 resumes the labelled poll block, which is
/// also the first activation's entry, as `lower_to_simple` projects it.
fn poll_function(name: &str, body: Vec<OpIR>) -> FunctionIR {
    let mut ops = vec![
        simple_op(DROP_INSERTED_ATTR, None, None, None),
        OpIR {
            state_targets: Some(vec![(RESUME_STATE, RESUME_LABEL)]),
            ..simple_op("state_switch", None, None, None)
        },
        simple_op("jump", None, None, Some(RESUME_LABEL)),
        simple_op("state_label", None, None, Some(RESUME_LABEL)),
    ];
    ops.extend(body);
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: vec!["self".into(), "future".into(), "polled".into()],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

/// The normalized wait: save the resume state, test the poll result, then a
/// pending exit that registers the wait and returns the result and a ready
/// exit that saves the running state, which nothing dispatches to, and returns
/// the result.
fn normalized_wait() -> FunctionIR {
    poll_function(
        "activation_exits",
        vec![
            simple_op("state_set", Some(&[]), None, Some(RESUME_STATE)),
            simple_op("is_pending", Some(&["polled"]), Some("waiting"), None),
            simple_op("br_if", Some(&["waiting"]), None, Some(WAIT_LABEL)),
            simple_op("state_set", Some(&[]), None, Some(RUNNING_STATE)),
            simple_op("ret", Some(&["polled"]), None, None),
            simple_op("label", None, None, Some(WAIT_LABEL)),
            simple_op("task_wait", Some(&["future"]), None, None),
            simple_op("ret", Some(&["polled"]), None, None),
        ],
    )
}

/// The instructions and root parameters `value` is computed from, through
/// aliases and block transport.
fn provenance(function: &Function, value: Value) -> (BTreeSet<Inst>, BTreeSet<Value>) {
    let (mut insts, mut roots) = (BTreeSet::new(), BTreeSet::new());
    let mut stack = vec![value];
    let mut seen = BTreeSet::new();
    while let Some(value) = stack.pop() {
        for source in canonical_value_sources(function, value) {
            if !seen.insert(source) {
                continue;
            }
            match function.dfg.value_def(source) {
                ValueDef::Result(inst, _) => {
                    insts.insert(inst);
                    stack.extend(function.dfg.inst_args(inst).iter().copied());
                }
                _ => {
                    roots.insert(source);
                }
            }
        }
    }
    (insts, roots)
}

fn instructions(function: &Function) -> impl Iterator<Item = Inst> + '_ {
    function
        .layout
        .blocks()
        .flat_map(|block| function.layout.block_insts(block))
}

#[test]
fn activation_exits_use_the_frame_the_exact_sentinel_and_one_return_owner() {
    let compiled =
        compile_function_to_clif_with_imports(vec![normalized_wait()], "activation_exits");
    let function = &compiled.function;
    let entry = function.layout.entry_block().expect("entry block");
    let params = function.dfg.block_params(entry).to_vec();
    let calls = |symbol: &str| {
        compiled
            .import_ids
            .get(symbol)
            .map(|&id| call_sites_for_import(function, id))
            .unwrap_or_default()
    };

    // StateSet saves each constant state through the frame parameter,
    // including a running state that no resume dispatches to.
    let mut saved = Vec::new();
    for (_, save) in calls("molt_obj_set_state") {
        let save_args = function.dfg.inst_args(save);
        assert!(
            value_originates_only_from(function, save_args[0], params[SELF]),
            "{}",
            function.display()
        );
        saved.push(constant(function, save_args[1]));
    }
    saved.sort();
    assert_eq!(
        saved,
        [Some(RESUME_STATE), Some(RUNNING_STATE)],
        "{}",
        function.display()
    );

    // TaskWait registers the frame and the future's object address, not its
    // boxed word.
    let &[(wait_block, register)] = calls("molt_sleep_register").as_slice() else {
        panic!("one wait registration:\n{}", function.display());
    };
    let register_args = function.dfg.inst_args(register);
    assert!(
        value_originates_only_from(function, register_args[0], params[SELF]),
        "{}",
        function.display()
    );
    let (address_insts, address_roots) = provenance(function, register_args[1]);
    assert_eq!(
        address_roots,
        BTreeSet::from([params[FUTURE]]),
        "{}",
        function.display()
    );
    assert!(
        address_insts
            .iter()
            .any(|&inst| function.dfg.insts[inst].opcode() == Opcode::Band),
        "the future word is masked to its object address:\n{}",
        function.display()
    );

    // IsPending is one exact comparison of the poll word with the sentinel,
    // and the branch taken into the wait tests it without any call.
    let comparisons: Vec<Inst> = instructions(function)
        .filter(|&inst| {
            function
                .dfg
                .inst_results(inst)
                .first()
                .is_some_and(|&result| {
                    comparison_operand(
                        function,
                        result,
                        IntCC::Equal,
                        molt_codegen_abi::pending_bits(),
                    )
                    .is_some_and(|word| value_originates_only_from(function, word, params[POLLED]))
                })
        })
        .collect();
    let &[comparison] = comparisons.as_slice() else {
        panic!("one exact pending comparison:\n{}", function.display());
    };
    let branches: Vec<Inst> = instructions(function)
        .filter(|&inst| {
            matches!(
                &function.dfg.insts[inst],
                InstructionData::Brif { blocks, .. }
                    if blocks[0].block(&function.dfg.value_lists) == wait_block
            )
        })
        .collect();
    let &[branch] = branches.as_slice() else {
        panic!(
            "the wait is the taken edge of one branch:\n{}",
            function.display()
        );
    };
    let (condition_insts, _) = provenance(function, function.dfg.inst_args(branch)[0]);
    assert!(
        condition_insts.contains(&comparison),
        "{}",
        function.display()
    );
    assert!(
        condition_insts
            .iter()
            .all(|&inst| !function.dfg.insts[inst].opcode().is_call()),
        "the pending condition needs no runtime call:\n{}",
        function.display()
    );
    assert!(calls("molt_is_truthy").is_empty(), "{}", function.display());

    // Both exits transfer the poll owner that TIR placed: no retain or release
    // is emitted, and the returned word is the poll result itself.
    assert!(
        calls("molt_inc_ref_obj").is_empty(),
        "{}",
        function.display()
    );
    assert!(
        calls("molt_dec_ref_obj").is_empty(),
        "{}",
        function.display()
    );
    let returns: Vec<Inst> = instructions(function)
        .filter(|&inst| function.dfg.insts[inst].opcode() == Opcode::Return)
        .collect();
    let &[ret] = returns.as_slice() else {
        panic!("one shared return:\n{}", function.display());
    };
    assert!(
        value_originates_only_from(function, function.dfg.inst_args(ret)[0], params[POLLED]),
        "{}",
        function.display()
    );
}

#[test]
#[should_panic(expected = "must expose it as explicit activation exits")]
fn drop_inserted_codegen_rejects_a_hidden_suspension() {
    let function = poll_function(
        "hidden_suspension",
        vec![
            simple_op("state_yield", Some(&["polled"]), None, Some(RESUME_STATE)),
            simple_op("state_label", None, None, Some(WAIT_LABEL)),
            simple_op("ret", Some(&["polled"]), None, None),
        ],
    );
    molt_ir::ir_schema::validate_state_dispatch(&function.ops)
        .expect("hidden suspension fixture must have valid state dispatch");
    compile_function_to_clif_with_imports(vec![function], "hidden_suspension");
}

#[test]
fn runtime_poll_uses_canonical_abi_without_a_second_python_recursion_boundary() {
    let mut function = normalized_wait();
    function.name = "runtime_poll_boundary".into();
    function.ops = vec![
        simple_op(DROP_INSERTED_ATTR, None, None, None),
        OpIR {
            s_value: Some("molt_future_poll".into()),
            ..simple_op("call", Some(&["future"]), Some("value"), None)
        },
        simple_op("ret", Some(&["value"]), None, None),
    ];
    let compiled = compile_function_to_clif_with_imports(vec![function], "runtime_poll_boundary");
    let poll = compiled.import_ids["molt_future_poll"];
    assert_eq!(call_sites_for_import(&compiled.function, poll).len(), 1);
    for name in [
        "molt_recursion_enter_fast",
        "molt_recursion_exit_fast",
        "molt_raise_recursion_error",
    ] {
        assert!(
            compiled
                .import_ids
                .get(name)
                .is_none_or(|&id| call_sites_for_import(&compiled.function, id).is_empty()),
            "runtime polling invented a Python activation: {}",
            compiled.function.display()
        );
    }
}
