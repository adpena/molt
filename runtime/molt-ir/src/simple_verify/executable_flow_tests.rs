use super::tests::op;
use super::{EdgeRole, SimpleIrExecutableFlow, simple_ir_executable_flow};
use crate::ir::OpIR;
use std::collections::BTreeSet;

fn assert_edges(flow: &SimpleIrExecutableFlow, source: usize, expected: &[(usize, EdgeRole)]) {
    let actual: Vec<_> = flow.edges[source]
        .iter()
        .map(|edge| (edge.target, edge.role))
        .collect();
    assert_eq!(actual, expected, "operation {source}");
    for edge in &flow.edges[source] {
        assert_eq!(edge.source, source);
        assert!(edge.target < flow.edges.len());
        assert!(edge.executable);
        assert_eq!(edge.role, edge.execution_role);
    }
}

#[test]
fn executable_roles_preserve_observers_and_loop_latches_without_try_metadata() {
    // The shared verifier fixture places the observer's normal successor at an
    // IF join, where PHI ordering annotates that edge as BranchTrue. Execution
    // still observes pending state once and then takes Normal or Exception.
    let labelled = |kind: &str| OpIR {
        value: Some(100),
        ..op(kind)
    };
    for observer in ["check_exception", "async_work_poll"] {
        let ops = vec![
            labelled("try_start"),
            op("loop_start"),
            OpIR {
                args: Some(vec!["condition".into()]),
                ..op("if")
            },
            labelled(observer),
            op("end_if"),
            op("loop_end"),
            labelled("label"),
            op("ret_void"),
        ];
        let flow = simple_ir_executable_flow(&ops);
        assert_eq!(flow.edges.len(), 8);
        assert_edges(&flow, 0, &[(1, EdgeRole::Normal)]);
        assert_edges(
            &flow,
            2,
            &[(3, EdgeRole::BranchTrue), (4, EdgeRole::BranchFalse)],
        );
        assert_edges(&flow, 3, &[(4, EdgeRole::Normal), (6, EdgeRole::Exception)]);
        assert_edges(&flow, 5, &[(1, EdgeRole::LoopLatch)]);
        assert_eq!(
            flow.edges[5][0].ordinal, 1,
            "filtering verifier-only exits must retain original edge identity"
        );
        assert!(flow.implicit_exits.is_empty());
    }
}

#[test]
fn implicit_exits_survive_clipping_real_conditional_and_exception_successors() {
    for observer in ["br_if", "check_exception", "async_work_poll"] {
        let ops = vec![
            OpIR {
                value: Some(5),
                ..op("label")
            },
            OpIR {
                value: Some(5),
                args: Some(vec!["condition".into()]),
                ..op(observer)
            },
        ];
        let flow = simple_ir_executable_flow(&ops);
        let taken = if observer == "br_if" {
            EdgeRole::BranchTrue
        } else {
            EdgeRole::Exception
        };
        assert_edges(&flow, 0, &[(1, EdgeRole::Fallthrough)]);
        assert_edges(&flow, 1, &[(0, taken)]);
        assert_eq!(flow.implicit_exits, BTreeSet::from([1]));
    }

    let flow = simple_ir_executable_flow(&[
        op("loop_start"),
        op("loop_break_if_exception"),
        op("loop_end"),
    ]);
    assert_edges(&flow, 1, &[(2, EdgeRole::BranchFalse)]);
    assert_edges(&flow, 2, &[(0, EdgeRole::LoopLatch)]);
    assert_eq!(flow.implicit_exits, BTreeSet::from([1]));
}

#[test]
fn explicit_resume_maps_override_source_inference_including_an_empty_map() {
    for (targets, expected) in [
        (
            Some(vec![(7, 200), (9, 200)]),
            vec![
                (1, EdgeRole::DispatchDefault),
                (4, EdgeRole::Resume),
                (4, EdgeRole::Resume),
            ],
        ),
        (Some(vec![]), vec![(1, EdgeRole::DispatchDefault)]),
        (
            None,
            vec![(1, EdgeRole::DispatchDefault), (2, EdgeRole::Resume)],
        ),
    ] {
        // Two canonical state IDs may select one label. The old source label
        // remains different, so inferred targets cannot satisfy this oracle.
        let ops = vec![
            OpIR {
                state_targets: targets,
                ..op("state_switch")
            },
            op("ret_void"),
            OpIR {
                value: Some(7),
                ..op("state_label")
            },
            OpIR {
                value: Some(7),
                args: Some(vec!["value".into()]),
                ..op("state_yield")
            },
            OpIR {
                value: Some(200),
                ..op("label")
            },
            op("ret_void"),
        ];
        let flow = simple_ir_executable_flow(&ops);
        assert_edges(&flow, 0, &expected);
        assert_edges(&flow, 3, &[]);
        assert!(flow.implicit_exits.is_empty());
    }
}

#[test]
fn saved_states_without_dispatch_do_not_invent_resume_edges() {
    let ops = vec![
        OpIR {
            value: Some(7),
            ..op("state_label")
        },
        OpIR {
            value: Some(7),
            ..op("state_set")
        },
        OpIR {
            value: Some(7),
            args: Some(vec!["value".into()]),
            ..op("state_yield")
        },
    ];
    let flow = simple_ir_executable_flow(&ops);
    assert_edges(&flow, 0, &[(1, EdgeRole::Fallthrough)]);
    assert_edges(&flow, 1, &[(2, EdgeRole::Fallthrough)]);
    assert_edges(&flow, 2, &[]);
    assert!(flow.implicit_exits.is_empty());
}

#[test]
fn entry_reachability_excludes_unreachable_latches_and_keeps_real_self_loops() {
    let loop_with_continue = simple_ir_executable_flow(&[
        op("loop_start"),
        op("loop_continue"),
        op("loop_end"),
        op("ret_void"),
    ]);
    assert_edges(&loop_with_continue, 2, &[(0, EdgeRole::LoopLatch)]);
    assert_eq!(
        loop_with_continue.executable_reachable(),
        vec![true, true, false, false],
        "the unconditional continue makes the lexical latch unreachable"
    );

    // One basic block loops to itself. Its unreachable tail is an implicit
    // exit source, not another entry root or an out-of-range target vertex.
    let self_loop = simple_ir_executable_flow(&[
        OpIR {
            value: Some(11),
            ..op("label")
        },
        OpIR {
            value: Some(11),
            ..op("jump")
        },
        op("nop"),
    ]);
    assert_eq!(self_loop.implicit_exits, BTreeSet::from([2]));
    assert_eq!(self_loop.executable_reachable(), vec![true, true, false]);
    assert!(
        simple_ir_executable_flow(&[])
            .executable_reachable()
            .is_empty()
    );
}

#[test]
fn empty_return_and_falloff_have_distinct_exit_boundaries() {
    let empty = simple_ir_executable_flow(&[]);
    assert!(empty.edges.is_empty());
    assert!(empty.implicit_exits.is_empty());

    let returned = simple_ir_executable_flow(&[op("ret_void")]);
    assert_edges(&returned, 0, &[]);
    assert!(returned.implicit_exits.is_empty());

    let unreachable_tail = simple_ir_executable_flow(&[op("ret_void"), op("nop")]);
    assert_edges(&unreachable_tail, 0, &[]);
    assert_eq!(unreachable_tail.implicit_exits, BTreeSet::from([1]));

    let falloff = simple_ir_executable_flow(&[op("nop")]);
    assert_edges(&falloff, 0, &[]);
    assert_eq!(falloff.implicit_exits, BTreeSet::from([0]));

    let dispatch = simple_ir_executable_flow(&[OpIR {
        state_targets: Some(vec![]),
        ..op("state_switch")
    }]);
    assert_edges(&dispatch, 0, &[]);
    assert_eq!(dispatch.implicit_exits, BTreeSet::from([0]));
}
