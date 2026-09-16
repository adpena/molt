//! Execute the emitted CFG, not an opcode-presence proxy.
use super::execution_support::executable_module;
use super::*;
use crate::tir::blocks::BlockId;
use crate::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

fn v(id: u32, repr: LirRepr) -> LirValue {
    LirValue {
        id: ValueId(id),
        ty: match repr {
            LirRepr::I64 => TirType::I64,
            LirRepr::Bool1 => TirType::Bool,
            LirRepr::F64 => TirType::F64,
            _ => TirType::DynBox,
        },
        repr,
    }
}
fn op(opcode: OpCode, args: &[u32], result: LirValue) -> LirOp {
    LirOp {
        tir_op: TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: args.iter().copied().map(ValueId).collect(),
            results: vec![result.id],
            attrs: AttrDict::new(),
            source_span: None,
        },
        result_values: vec![result],
    }
}
fn constant(id: u32, number: i64, repr: LirRepr) -> LirOp {
    let mut value = op(OpCode::ConstInt, &[], v(id, repr));
    value
        .tir_op
        .attrs
        .insert("value".into(), AttrValue::Int(number));
    value
}
fn branch(target: u32, args: &[u32]) -> LirTerminator {
    LirTerminator::Branch {
        target: BlockId(target),
        args: args.iter().copied().map(ValueId).collect(),
    }
}
fn cond(value: u32, yes: u32, yes_args: &[u32], no: u32, no_args: &[u32]) -> LirTerminator {
    LirTerminator::CondBranch {
        cond: ValueId(value),
        then_block: BlockId(yes),
        then_args: yes_args.iter().copied().map(ValueId).collect(),
        else_block: BlockId(no),
        else_args: no_args.iter().copied().map(ValueId).collect(),
    }
}
fn ret(value: u32) -> LirTerminator {
    LirTerminator::Return {
        values: vec![ValueId(value)],
    }
}
fn block(id: u32, args: Vec<LirValue>, mut ops: Vec<LirOp>, terminator: LirTerminator) -> LirBlock {
    // Tiny boxed constants pass through the genuine dynamic truthiness import,
    // giving the test an exact visit sequence without replacing CFG semantics.
    let marker = 100 + id * 2;
    ops.insert(
        0,
        op(OpCode::Bool, &[marker], v(marker + 1, LirRepr::Bool1)),
    );
    ops.insert(0, constant(marker, i64::from(id), LirRepr::DynBox));
    LirBlock {
        id: BlockId(id),
        args,
        ops,
        terminator,
    }
}
fn cfg(name: &str, result: TirType, blocks: Vec<LirBlock>) -> LirFunction {
    let entry = &blocks[0];
    LirFunction {
        name: name.into(),
        param_names: entry.args.iter().map(|v| format!("v{}", v.id.0)).collect(),
        param_types: entry.args.iter().map(|v| v.ty.clone()).collect(),
        return_types: vec![result],
        entry_block: entry.id,
        blocks: blocks.into_iter().map(|b| (b.id, b)).collect(),
        label_id_map: HashMap::new(),
    }
}

fn entry_rotation() -> LirFunction {
    use LirRepr::*;
    cfg(
        "entry_rotation",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, I64), v(2, I64), v(3, I64)],
                vec![op(OpCode::Not, &[0], v(4, Bool1))],
                cond(0, 0, &[4, 2, 3, 1], 1, &[3]),
            ),
            block(1, vec![v(5, I64)], vec![], ret(5)),
        ],
    )
}
fn mixed_swap() -> LirFunction {
    use LirRepr::*;
    cfg(
        "mixed_swap",
        TirType::F64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, F64), v(2, F64)],
                vec![op(OpCode::Not, &[0], v(3, Bool1))],
                cond(0, 0, &[3, 2, 1], 1, &[2]),
            ),
            block(1, vec![v(4, F64)], vec![], ret(4)),
        ],
    )
}
fn switch_self() -> LirFunction {
    use LirRepr::*;
    cfg(
        "switch_self",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, I64), v(1, I64)],
                vec![],
                LirTerminator::Switch {
                    value: ValueId(0),
                    cases: vec![
                        (0, BlockId(0), vec![ValueId(1), ValueId(0)]),
                        (1, BlockId(1), vec![ValueId(0)]),
                    ],
                    default: BlockId(2),
                    default_args: vec![ValueId(0)],
                },
            ),
            block(1, vec![v(2, I64)], vec![], ret(2)),
            block(2, vec![v(3, I64)], vec![], ret(3)),
        ],
    )
}
fn same_target() -> LirFunction {
    use LirRepr::*;
    cfg(
        "same_target",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, I64), v(2, I64)],
                vec![],
                cond(0, 1, &[1], 1, &[2]),
            ),
            block(1, vec![v(3, I64)], vec![], ret(3)),
        ],
    )
}
fn forward_skip() -> LirFunction {
    use LirRepr::*;
    cfg(
        "forward_skip",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, I64)],
                vec![],
                cond(0, 1, &[], 3, &[1]),
            ),
            block(1, vec![], vec![], branch(2, &[])),
            block(2, vec![], vec![constant(3, 99, I64)], branch(3, &[3])),
            block(3, vec![v(2, I64)], vec![], ret(2)),
            block(99, vec![], vec![], LirTerminator::Unreachable),
        ],
    )
}
fn nested_exits() -> LirFunction {
    use LirRepr::*;
    cfg(
        "nested_exits",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, Bool1), v(2, I64), v(3, I64), v(4, I64)],
                vec![],
                branch(1, &[0, 3, 4]),
            ),
            block(
                1,
                vec![v(5, Bool1), v(6, I64), v(7, I64)],
                vec![op(OpCode::Not, &[5], v(8, Bool1))],
                branch(2, &[1]),
            ),
            block(
                2,
                vec![v(9, Bool1)],
                vec![op(OpCode::Not, &[9], v(10, Bool1))],
                cond(9, 2, &[10], 3, &[]),
            ),
            block(
                3,
                vec![],
                vec![],
                LirTerminator::Switch {
                    value: ValueId(2),
                    cases: vec![
                        (1, BlockId(4), vec![ValueId(6)]),
                        (2, BlockId(5), vec![ValueId(7)]),
                    ],
                    default: BlockId(6),
                    default_args: vec![],
                },
            ),
            block(4, vec![v(11, I64)], vec![], ret(11)),
            block(5, vec![v(12, I64)], vec![], ret(12)),
            block(6, vec![], vec![], cond(5, 1, &[8, 7, 6], 4, &[6])),
        ],
    )
}
fn inner_to_outer() -> LirFunction {
    use LirRepr::*;
    cfg(
        "inner_to_outer",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, Bool1), v(1, Bool1), v(2, I64), v(3, I64)],
                vec![],
                branch(1, &[0, 2, 3]),
            ),
            block(
                1,
                vec![v(4, Bool1), v(5, I64), v(6, I64)],
                vec![op(OpCode::Not, &[4], v(7, Bool1))],
                branch(2, &[1]),
            ),
            block(
                2,
                vec![v(8, Bool1)],
                vec![op(OpCode::Not, &[8], v(9, Bool1))],
                cond(4, 1, &[7, 6, 5], 3, &[]),
            ),
            block(3, vec![], vec![], cond(8, 2, &[9], 4, &[6])),
            block(4, vec![v(10, I64)], vec![], ret(10)),
        ],
    )
}

#[test]
fn reducible_cfg_executes_loop_scopes_and_selected_parallel_edges() {
    let Some(node) = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "LIR CFG execution",
    ) else {
        return;
    };
    let (directory, _cleanup) = wasm_test_temp_dir();
    let infinite = cfg(
        "single_self",
        TirType::I64,
        vec![block(0, vec![], vec![], branch(0, &[]))],
    );
    let mut modules = serde_json::Map::new();
    for function in [
        entry_rotation(),
        mixed_swap(),
        switch_self(),
        same_target(),
        forward_skip(),
        nested_exits(),
        inner_to_outer(),
        infinite,
    ] {
        let path = directory.join(format!("{}.wasm", function.name));
        fs::write(&path, executable_module(&lower_lir_to_wasm(&function))).unwrap();
        modules.insert(function.name, json!(path));
    }
    let config = directory.join("cfg-cases.json");
    fs::write(&config, serde_json::to_vec(&modules).unwrap()).unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const files = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
let events = [], stopAt = 0;
const sentinel = new Error('bounded self-loop observed');
const hooks = {
  is_truthy(value) {
    events.push(Number(value & ((1n << 47n) - 1n)));
    if (stopAt && events.length === stopAt) throw sentinel;
    return 1n;
  },
  inc_ref_obj() {}, dec_ref_obj() {}, exception_pending() { return 0n; },
};
const functions = Object.fromEntries(Object.entries(files).map(([name, path]) => [
  name, new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(path)), {molt_runtime: hooks}).exports.run
]));
function check(name, args, result, visits) {
  console.error('cfg-case: ' + name + ' args=' + args.map(String).join(','));
  events = []; stopAt = 0;
  assert.equal(functions[name](...args), result, name);
  assert.deepEqual(events, visits, name);
}
check('entry_rotation', [1, 11n, 22n, 33n], 11n, [0, 0, 1]);
check('entry_rotation', [0, 11n, 22n, 33n], 33n, [0, 1]);
check('mixed_swap', [1, 1.25, -3.5], 1.25, [0, 0, 1]);
check('mixed_swap', [0, 1.25, -3.5], -3.5, [0, 1]);
check('switch_self', [0n, 1n], 1n, [0, 0, 1]);
check('switch_self', [2n, 0n], 2n, [0, 2]);
check('same_target', [1, 11n, 22n], 11n, [0, 1]);
check('same_target', [0, 11n, 22n], 22n, [0, 1]);
check('forward_skip', [1, 7n], 99n, [0, 1, 2, 3]);
check('forward_skip', [0, 7n], 7n, [0, 3]);
check('nested_exits', [1, 1, 0n, 11n, 22n], 22n, [0, 1, 2, 2, 3, 6, 1, 2, 2, 3, 6, 4]);
check('nested_exits', [1, 1, 1n, 11n, 22n], 11n, [0, 1, 2, 2, 3, 4]);
check('nested_exits', [1, 1, 2n, 11n, 22n], 22n, [0, 1, 2, 2, 3, 5]);
check('inner_to_outer', [1, 1, 11n, 22n], 11n, [0, 1, 2, 1, 2, 3, 2, 3, 4]);
console.error('cfg-case: bounded single-block self-loop');
events = []; stopAt = 3;
assert.throws(() => functions.single_self(), error => error === sentinel);
assert.deepEqual(events, [0, 0, 0]);
"#,
        &[config.as_path()],
        "execute reducible CFG and parallel edge payloads",
    );
}

fn rejects(function: &LirFunction, expected: &str) {
    let error = match std::panic::catch_unwind(|| lower_lir_to_wasm(function)) {
        Ok(_) => panic!("invalid CFG admitted"),
        Err(error) => error,
    };
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains(expected),
        "{message:?} does not contain {expected:?}"
    );
}

#[test]
fn cfg_rejects_irreducible_multiple_entry_cycle() {
    let function = cfg(
        "irreducible",
        TirType::I64,
        vec![
            block(
                0,
                vec![v(0, LirRepr::Bool1), v(1, LirRepr::I64)],
                vec![],
                cond(0, 1, &[], 2, &[]),
            ),
            block(1, vec![], vec![], branch(2, &[])),
            block(2, vec![], vec![], cond(0, 1, &[], 3, &[])),
            block(3, vec![], vec![], ret(1)),
        ],
    );
    rejects(&function, "irreducible");
}

#[test]
fn cfg_rejects_missing_targets_and_inexact_edge_payloads() {
    let mut missing_entry = same_target();
    missing_entry.entry_block = BlockId(77);
    rejects(&missing_entry, "missing entry block");
    let mut missing_target = same_target();
    missing_target
        .blocks
        .get_mut(&BlockId(0))
        .unwrap()
        .terminator = branch(77, &[]);
    rejects(&missing_target, "branch target is missing");
    // Malformed dead blocks are still diagnosed; well-formed unreachable code
    // is omitted by the executed forward_skip fixture above.
    missing_target = same_target();
    missing_target
        .blocks
        .insert(BlockId(99), block(99, vec![], vec![], branch(77, &[])));
    rejects(&missing_target, "branch target is missing");
    let mut arity = same_target();
    arity.blocks.get_mut(&BlockId(0)).unwrap().terminator = branch(1, &[]);
    rejects(&arity, "edge argument arity");
    let mut repr = same_target();
    repr.blocks.get_mut(&BlockId(1)).unwrap().args[0].repr = LirRepr::DynBox;
    rejects(&repr, "edge argument representation");
    let mut missing_source = same_target();
    missing_source
        .blocks
        .get_mut(&BlockId(0))
        .unwrap()
        .terminator = branch(1, &[77]);
    rejects(&missing_source, "edge source value is missing");
}
