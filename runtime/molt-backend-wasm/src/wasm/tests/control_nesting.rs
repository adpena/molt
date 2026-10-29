//! HF-546: structured control nesting stays bounded in every non-linear
//! dispatch mode, independently of how many blocks and far targets a
//! function has.

use super::support::*;
use crate::wasm::state_dispatch::with_test_scope_shape;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};
use wasmparser::Operator;

/// Binaryen's `-Oz` rebuilds far-targeted block scopes as nested `if`s and
/// its stack writer recurses once per level: 2,128 levels overflowed a
/// 544 KiB macOS worker stack (HF-546). A flat function keeps at most 1,024
/// overlapping far-targeted scopes, a tree at most three levels of 128; the
/// return scope and selector trap blocks add a few more.
const FAR_NESTING_BOUND: usize = 1024 + 16;
/// WABT's binary reader rejects 16,384 nested labels. A flat function keeps
/// at most 8,192 block scopes, inside at most 16 frame, dispatch-loop and
/// conditional wrapper levels.
const EMITTED_NESTING_BOUND: usize = 8192 + 16;
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

/// The most far-targeted `block` scopes open at once in any body, measured
/// from the bytes alone. A branch is far when another `block` lies between it
/// and its target `block` (a fallthrough `br` to the innermost block, even from
/// inside an `if` arm, is adjacent). A far-targeted scope counts from its
/// first far branch to its `end`: the nested `if` an optimizer can rebuild.
fn max_far_scope_nesting(wasm: &[u8]) -> usize {
    let mut deepest = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CodeSectionEntry(body) = payload.unwrap() else {
            continue;
        };
        // (label id, is block) for each open structure.
        let mut stack: Vec<(usize, bool)> = Vec::new();
        let mut first_far: Vec<Option<usize>> = Vec::new();
        let mut ends: Vec<usize> = Vec::new();
        let far_branch =
            |stack: &[(usize, bool)], depth: u32, at: usize, first_far: &mut Vec<Option<usize>>| {
                let target = stack.len() - 1 - depth as usize;
                let (id, is_block) = stack[target];
                if is_block && stack[target + 1..].iter().any(|&(_, block)| block) {
                    first_far[id] = Some(first_far[id].map_or(at, |first| first.min(at)));
                }
            };
        for (at, op) in body.get_operators_reader().unwrap().into_iter().enumerate() {
            match op.unwrap() {
                Operator::Block { .. } => {
                    stack.push((first_far.len(), true));
                    first_far.push(None);
                    ends.push(usize::MAX);
                }
                Operator::Loop { .. } | Operator::If { .. } | Operator::TryTable { .. } => {
                    stack.push((first_far.len(), false));
                    first_far.push(None);
                    ends.push(usize::MAX);
                }
                Operator::End => {
                    if let Some((id, _)) = stack.pop() {
                        ends[id] = at;
                    }
                }
                Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => {
                    far_branch(&stack, relative_depth, at, &mut first_far);
                }
                Operator::BrTable { targets } => {
                    for depth in targets
                        .targets()
                        .chain(std::iter::once(Ok(targets.default())))
                    {
                        far_branch(&stack, depth.unwrap(), at, &mut first_far);
                    }
                }
                _ => {}
            }
        }
        let mut events: Vec<(usize, isize)> = Vec::new();
        for (id, first) in first_far.iter().enumerate() {
            if let Some(first) = *first {
                events.push((first, 1));
                events.push((ends[id], -1));
            }
        }
        events.sort();
        let mut open = 0isize;
        for (_, change) in events {
            open += change;
            deepest = deepest.max(open as usize);
        }
    }
    deepest
}

fn count_ops(wasm: &[u8], matches: impl Fn(&Operator<'_>) -> bool) -> usize {
    let mut count = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            for op in body.get_operators_reader().unwrap() {
                count += usize::from(matches(&op.unwrap()));
            }
        }
    }
    count
}

fn loop_count(wasm: &[u8]) -> usize {
    count_ops(wasm, |op| matches!(op, Operator::Loop { .. }))
}

/// A forward-only flat function has no selector; the tree selects with
/// `br_table`.
fn uses_scope_tree(wasm: &[u8]) -> bool {
    count_ops(wasm, |op| matches!(op, Operator::BrTable { .. })) > 0
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
/// each pad holds two (`label` is a block of its own): `3 * count + 2` blocks,
/// and all `count` pads are far targets open after the last statement.
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

/// Many statements share one pad: few far targets, `count + 4` blocks.
fn shared_pad_guest(count: i64) -> Vec<u8> {
    let mut ops = vec![marked("label", 0)];
    for line in 1..=count {
        ops.push(marked("line", line));
        ops.push(marked("check_exception", PAD_LINE));
    }
    ops.extend([
        wasm_test_op("ret_void", None, vec![]),
        marked("label", PAD_LINE),
        marked("line", PAD_LINE),
        wasm_test_op("ret_void", None, vec![]),
    ]);
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
    // 1,200 pads open at once exceed the production flat limit of 1,024.
    const FORWARD: i64 = 1_200;
    const HEAD: i64 = 1_100;
    const TAIL: i64 = 100;
    const STATE: i64 = 7;
    let guests = || {
        [
            ("forward", forward_guest(FORWARD)),
            ("dispatch", dispatch_guest(HEAD, TAIL)),
            ("stateful", stateful_guest(FORWARD, STATE)),
        ]
    };
    // Production shape: two levels of 128 for these 3,602-block guests. A
    // shape of four reaches six levels in the same guests, so every selector
    // depth executes.
    let mut cases = Vec::new();
    for (name, wasm) in guests() {
        cases.push((format!("{name}-128"), wasm));
    }
    for (name, wasm) in with_test_scope_shape(8, 64, 2, guests) {
        cases.push((format!("{name}-4"), wasm));
    }
    for (name, wasm) in &cases {
        wasmparser::Validator::new().validate_all(wasm).unwrap();
        let far = max_far_scope_nesting(wasm);
        assert!(
            far <= FAR_NESTING_BOUND,
            "{name}: {far} far-targeted scopes overlap, above {FAR_NESTING_BOUND}"
        );
        let depth = max_control_depth(wasm);
        assert!(
            depth <= EMITTED_NESTING_BOUND,
            "{name}: control nests {depth} deep, above {EMITTED_NESTING_BOUND}"
        );
        if name.starts_with("forward") {
            assert_eq!(
                loop_count(wasm),
                0,
                "{name}: forward routing between scope nodes must not add a loop"
            );
        }
    }
    // Only a measured excess leaves the flat layout: 1,024 open pads stay
    // flat, 1,025 take the tree. With limits of 8 far scopes and 64 blocks,
    // either limit alone moves a function.
    for (wasm, tree, label) in [
        (forward_guest(1_024), false, "1,024 pads"),
        (forward_guest(1_025), true, "1,025 pads"),
        (
            with_test_scope_shape(8, 64, 2, || forward_guest(8)),
            false,
            "8 pads",
        ),
        (
            with_test_scope_shape(8, 64, 2, || forward_guest(9)),
            true,
            "9 pads",
        ),
        (
            with_test_scope_shape(8, 64, 2, || shared_pad_guest(60)),
            false,
            "64 blocks",
        ),
        (
            with_test_scope_shape(8, 64, 2, || shared_pad_guest(61)),
            true,
            "65 blocks",
        ),
    ] {
        assert_eq!(uses_scope_tree(&wasm), tree, "{label}");
        let far = max_far_scope_nesting(&wasm);
        assert!(far <= FAR_NESTING_BOUND, "{label}: {far} far scopes");
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
    1023, 1024, 1025, config.head, config.head + 1, total].filter((failure) => failure <= total);
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
