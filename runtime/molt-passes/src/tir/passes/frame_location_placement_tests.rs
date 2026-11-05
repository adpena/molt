use super::*;
use crate::ir::FunctionIR;

/// A synchronous frame with one parameter home (slot 0) and a function-level
/// exception label 9 that exits the frame.
fn lift(context: &str, ops: serde_json::Value) -> TirFunction {
    let mut all = vec![
        serde_json::json!({"kind": "trace_enter_slot", "s_value": "f"}),
        serde_json::json!({"kind": "check_exception", "exception_edge": "origin", "value": 9}),
    ];
    all.extend(ops.as_array().expect("op list").iter().cloned());
    all.extend([
        serde_json::json!({"kind": "trace_exit"}),
        serde_json::json!({"kind": "ret_void"}),
        serde_json::json!({"kind": "label", "value": 9}),
        serde_json::json!({"kind": "trace_exit"}),
        serde_json::json!({"kind": "ret_void"}),
    ]);
    let ir: FunctionIR = serde_json::from_value(serde_json::json!({
        "return_abi": "void", "name": "f", "execution_context": context,
        "params": ["p"], "param_types": ["Any"], "ops": all,
    }))
    .unwrap();
    crate::tir::lower_from_simple::lower_to_tir(&ir)
}

fn publications(func: &TirFunction) -> Vec<i64> {
    let mut found: Vec<i64> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::FrameSetLocation)
        .filter_map(|op| match op.attrs.get("value") {
            Some(AttrValue::Int(location)) => Some(*location),
            _ => None,
        })
        .collect();
    found.sort_unstable();
    found
}

fn origin_locations(func: &TirFunction) -> Vec<Option<i64>> {
    let mut found: Vec<(u32, usize, Option<i64>)> = func
        .blocks
        .values()
        .flat_map(|block| {
            block
                .ops
                .iter()
                .enumerate()
                .filter(|(_, op)| is_origin_check(op))
                .map(move |(index, op)| (block.id.0, index, op.frame_location()))
        })
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, _, location)| location).collect()
}

/// Exact-int work that cannot call Python keeps no publication. Each Origin
/// check materializes the publication that reached it; the return keeps its own.
#[test]
fn exact_int_work_keeps_only_the_terminal_publication() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "const", "value": 1, "out": "x"},
            {"kind": "const", "value": 2, "out": "y"},
            {"kind": "frame_set_location", "value": 3},
            {"kind": "add", "args": ["x", "y"], "out": "z"},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 4},
            {"kind": "frame_home_store", "args": ["z"], "out": "bound", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 5},
        ]),
    );
    let stats = run(&mut func);
    assert_eq!(publications(&func), vec![5]);
    assert_eq!(stats.ops_removed, 2);
    let locations = origin_locations(&func);
    assert!(
        locations.contains(&Some(3)) && locations.contains(&Some(4)),
        "{locations:?}"
    );
}

/// A call can run Python that reads the frame: its publication stays.
#[test]
fn a_call_keeps_its_publication() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "frame_set_location", "value": 3},
            {"kind": "call", "s_value": "work", "args": [], "out": "r"},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
        ]),
    );
    run(&mut func);
    assert_eq!(publications(&func), vec![3]);
}

/// Rebinding a home that holds an exact int releases nothing that can
/// finalize; rebinding one that holds an unknown value is a read.
#[test]
fn binding_release_reads_only_an_unproven_previous_binding() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "const", "value": 1, "out": "x"},
            {"kind": "frame_set_location", "value": 3},
            {"kind": "frame_home_store", "args": ["x"], "out": "a", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 4},
            {"kind": "frame_home_store", "args": ["x"], "out": "b", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 5},
            {"kind": "frame_home_store", "args": ["p"], "out": "c", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 6},
            {"kind": "frame_home_store", "args": ["x"], "out": "d", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
        ]),
    );
    run(&mut func);
    // 3 and 4 release unbound and an int; 5 releases an int; 6 releases the
    // parameter, which may own a finalizer. The terminal return reads 6.
    assert_eq!(publications(&func), vec![6]);
}

/// A resumed or inherited activation proves nothing about its homes.
#[test]
fn an_inherited_activation_reads_its_first_binding_release() {
    let mut func = lift(
        "inherited",
        serde_json::json!([
            {"kind": "const", "value": 1, "out": "x"},
            {"kind": "frame_set_location", "value": 3},
            {"kind": "frame_home_store", "args": ["x"], "out": "a", "value": 1},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 4},
        ]),
    );
    run(&mut func);
    assert_eq!(publications(&func), vec![3, 4]);
}

/// An Origin check that two different publications reach cannot name one
/// site: it stays a reader, so both publications stay.
#[test]
fn an_origin_check_with_two_reaching_sites_stays_a_reader() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "const", "value": 1, "out": "x"},
            {"kind": "if", "args": ["p"]},
            {"kind": "frame_set_location", "value": 3},
            {"kind": "else"},
            {"kind": "frame_set_location", "value": 4},
            {"kind": "end_if"},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 5},
        ]),
    );
    run(&mut func);
    assert_eq!(publications(&func), vec![3, 4, 5]);
    assert!(origin_locations(&func).contains(&None));
}

/// An Unwind edge forwards the word to its handler: a reader there keeps the
/// publication that reaches the edge.
#[test]
fn an_unwind_edge_forwards_liveness_to_its_handler() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "frame_set_location", "value": 3},
            {"kind": "check_exception", "exception_edge": "unwind", "value": 9},
            {"kind": "frame_set_location", "value": 4},
        ]),
    );
    run(&mut func);
    // Label 9 exits the frame (trace_exit, return): it reads 3.
    assert_eq!(publications(&func), vec![3, 4]);
}

/// The pass only deletes publications and adds attrs: a second run is a no-op.
#[test]
fn placement_is_idempotent() {
    let mut func = lift(
        "local",
        serde_json::json!([
            {"kind": "const", "value": 1, "out": "x"},
            {"kind": "frame_set_location", "value": 3},
            {"kind": "add", "args": ["x", "x"], "out": "z"},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
            {"kind": "frame_set_location", "value": 4},
            {"kind": "call", "s_value": "work", "args": [], "out": "r"},
            {"kind": "check_exception", "exception_edge": "origin", "value": 9},
        ]),
    );
    run(&mut func);
    let first = (publications(&func), origin_locations(&func));
    let again = run(&mut func);
    assert_eq!((publications(&func), origin_locations(&func)), first);
    assert_eq!(again.total_changes(), 0);
}
