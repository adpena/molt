//! HF-546: structured control nesting stays bounded in every non-linear
//! dispatch mode, independently of how many physical blocks a function has.

use super::support::*;
use crate::wasm::state_dispatch::with_test_scope_fanout_log2;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};
use wasmparser::Operator;

/// Consumers cap or recurse on nesting depth: WABT's binary reader rejects
/// 16,384 nested labels, and Binaryen's stack writer overflowed a 544 KiB
/// macOS worker stack at 2,128 levels (HF-546). A function keeps at most 512
/// block scopes (flat up to 512 blocks, a radix-128 tree beyond), inside at
/// most 16 frame, dispatch-loop and conditional wrapper levels. The layout it
/// replaced nested one scope per block: about 800 for the tree guests here.
const NESTING_BOUND: usize = 512 + 16;
/// The line that a statement's own cleanup pad records.
const PAD_LINE: i64 = 1_000_000;

fn marked(kind: &str, value: i64) -> OpIR {
    OpIR {
        value: Some(value),
        ..wasm_test_op(kind, None, vec![])
    }
}

fn emit(function: FunctionIR) -> Vec<u8> {
    wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(
        SimpleIR {
            functions: vec![function],
            profile: None,
        },
        Default::default(),
    )
    .wasm
}

/// Decode every body and count structured openers against `end`, without
/// consulting the emitter's scope plan.
fn max_control_depth(wasm: &[u8]) -> usize {
    let mut deepest = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            let mut depth = 0usize;
            for op in body.get_operators_reader().unwrap() {
                match op.unwrap() {
                    Operator::Block { .. }
                    | Operator::Loop { .. }
                    | Operator::If { .. }
                    | Operator::TryTable { .. } => {
                        depth += 1;
                        deepest = deepest.max(depth);
                    }
                    // The function body's own `end` has no opener.
                    Operator::End => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }
    deepest
}

fn loop_count(wasm: &[u8]) -> usize {
    let mut loops = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            for op in body.get_operators_reader().unwrap() {
                loops += usize::from(matches!(op.unwrap(), Operator::Loop { .. }));
            }
        }
    }
    loops
}

/// The HF-546 shape: each statement check leaves for its own cleanup pad, and
/// every pad follows the whole body, so all pads are live forward targets.
fn statements(first_line: i64, count: i64, body: &mut Vec<OpIR>, pads: &mut Vec<OpIR>) {
    for line in first_line..first_line + count {
        body.push(marked("line", line));
        body.push(marked("check_exception", PAD_LINE + line));
        pads.push(marked("label", PAD_LINE + line));
        pads.push(marked("line", PAD_LINE + line));
        pads.push(wasm_test_op("ret_void", None, vec![]));
    }
}

/// Forward-only Jumpful: no dispatch loop. Each statement ends one block and
/// each pad holds two (`label` is a block of its own): `3 * count + 2` blocks.
fn forward_guest(count: i64) -> Vec<u8> {
    let (mut ops, mut pads) = (vec![marked("label", 0)], Vec::new());
    statements(1, count, &mut ops, &mut pads);
    ops.push(wasm_test_op("ret_void", None, vec![]));
    ops.extend(pads);
    emit(wasm_test_function("molt_main", vec![], None, ops))
}

/// Jumpful with a dispatch loop: a backward jump re-enters a block whose
/// radix digits are nonzero at every level.
fn dispatch_guest(head: i64, tail: i64) -> Vec<u8> {
    let (mut ops, mut pads) = (vec![marked("label", 0)], Vec::new());
    statements(1, head, &mut ops, &mut pads);
    ops.extend([
        marked("jump", 30),
        marked("label", 20),
        marked("line", 20),
        wasm_test_op("ret_void", None, vec![]),
        marked("label", 30),
    ]);
    statements(head + 1, tail, &mut ops, &mut pads);
    ops.push(marked("jump", 20));
    ops.extend(pads);
    emit(wasm_test_function("molt_main", vec![], None, ops))
}

/// Stateful: the saved state resumes at a label after every pad.
fn stateful_guest(count: i64, state: i64) -> Vec<u8> {
    let mut ops = vec![OpIR {
        state_targets: Some(vec![(state, state)]),
        ..wasm_test_op("state_switch", None, vec![])
    }];
    let mut pads = Vec::new();
    statements(1, count, &mut ops, &mut pads);
    ops.push(wasm_test_op("ret_void", None, vec![]));
    ops.extend(pads);
    ops.extend([
        marked("state_label", state),
        marked("line", 20),
        wasm_test_op("ret_void", None, vec![]),
    ]);
    emit(wasm_test_function("molt_main", vec!["task"], None, ops))
}

#[test]
fn control_nesting_stays_bounded_across_dispatch_modes() {
    const FORWARD: i64 = 400;
    const HEAD: i64 = 300;
    const TAIL: i64 = 100;
    const STATE: i64 = 7;
    let guests = || {
        [
            ("forward", forward_guest(FORWARD)),
            ("dispatch", dispatch_guest(HEAD, TAIL)),
            ("stateful", stateful_guest(FORWARD, STATE)),
        ]
    };
    // 512 blocks stay flat; 515 blocks take the tree at its smallest size.
    for (statements, flat) in [(170, true), (171, false)] {
        let wasm = forward_guest(statements);
        let depth = max_control_depth(&wasm);
        assert!(
            depth <= NESTING_BOUND,
            "{statements} statements nest {depth} deep"
        );
        assert_eq!(
            depth > 3 * statements as usize,
            flat,
            "{statements} statements nest {depth} deep"
        );
    }
    // Production shape: 128-way nodes above 512 blocks, two levels for these
    // 1,202-block guests. A shape of four reaches six levels in the same
    // guests, so every selector depth executes.
    let mut cases = Vec::new();
    for (name, wasm) in guests() {
        cases.push((format!("{name}-128"), wasm));
    }
    for (name, wasm) in with_test_scope_fanout_log2(2, guests) {
        cases.push((format!("{name}-4"), wasm));
    }
    for (name, wasm) in &cases {
        wasmparser::Validator::new().validate_all(wasm).unwrap();
        let depth = max_control_depth(wasm);
        assert!(
            depth <= NESTING_BOUND,
            "{name}: control nests {depth} deep, above {NESTING_BOUND}"
        );
        if name.starts_with("forward") {
            assert_eq!(
                loop_count(wasm),
                0,
                "{name}: forward routing between scope nodes must not add a loop"
            );
        }
    }

    let node = PathBuf::from("node");
    let (temp, _cleanup) = wasm_test_temp_dir();
    let mut entries = Vec::new();
    for (name, wasm) in &cases {
        let (memory_pages, table_entries) = wasm_import_minimums(wasm);
        let path = temp.join(format!("{name}.wasm"));
        fs::write(&path, wasm).unwrap();
        entries.push(json!({"name":name,"path":path,"memory_pages":memory_pages,"table_entries":table_entries}));
    }
    let config = temp.join("nesting.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases":entries,
            "none":molt_codegen_abi::box_none_bits().to_string(),
            "forward":FORWARD, "head":HEAD, "tail":TAIL, "state":STATE, "pad":PAD_LINE,
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none);
const range = (first, last) => Array.from({length: last - first + 1}, (_, i) => first + i);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  let lines, polls, failAt, savedState = 0n;
  const hooks = {
    trace_set_line(line) { lines.push(Number(line)); return none; },
    exception_pending() { return ++polls === failAt ? 1n : 0n; },
    obj_get_state(task) { assert.equal(task, 64n); return savedState; },
    obj_set_state() { throw Error('no state publication in this guest'); },
  };
  const imports = {env:{memory:new WebAssembly.Memory({initial:test.memory_pages}),
    __indirect_function_table:new WebAssembly.Table({initial:test.table_entries,element:'anyfunc'})}};
  for (const entry of WebAssembly.Module.imports(module)) {
    if (entry.kind !== 'function') { assert.ok(entry.name in imports.env); continue; }
    imports[entry.module] ??= {};
    imports[entry.module][entry.name] = hooks[entry.name] ?? (() => {throw Error('unexpected import '+entry.name);});
  }
  const app = new WebAssembly.Instance(module, imports).exports;
  const mode = test.name.split('-')[0];
  const total = mode === 'dispatch' ? config.head + config.tail : config.forward;
  const run = (failure, ...args) => {
    lines = []; polls = 0; failAt = failure;
    assert.equal(app.molt_main(...args), none);
  };
  // Statement s ends physical block s, so failures at radix boundaries of
  // both fan-outs leave from the first and last block of a node and must
  // reach exactly their own pad.
  const failures = [1, 2, 3, 4, 5, 15, 16, 17, 63, 64, 65, 127, 128, 129, 255, 256, 257,
    config.head, config.head + 1, total].filter((failure) => failure <= total);
  for (const failure of failures) {
    if (mode === 'stateful') run(failure, 64n); else run(failure);
    assert.deepEqual(lines, [...range(1, failure), config.pad + failure], `${test.name} fail@${failure}`);
    assert.equal(polls, failure);
  }
  if (mode === 'stateful') {
    run(0, 64n);
    assert.deepEqual(lines, range(1, total));
    savedState = BigInt(config.state);
    run(0, 64n);
    assert.deepEqual(lines, [20], 'saved state selects the far resume block');
    // An unplanned saved state traps before any observer runs.
    savedState = BigInt(config.state + 1);
    lines = []; polls = 0; failAt = 0;
    assert.throws(() => app.molt_main(64n), WebAssembly.RuntimeError);
    assert.deepEqual(lines, []);
  } else {
    run(0);
    assert.deepEqual(lines, mode === 'dispatch' ? [...range(1, total), 20] : range(1, total));
    assert.equal(polls, total);
  }
}
"#,
        &[&config],
        "bounded scope tree routes every transfer to its authored target",
    );
}
