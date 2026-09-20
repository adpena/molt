use super::execution_support::executable_module;
use super::*;
use crate::tir::blocks::BlockId;
use crate::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

fn value(id: u32, repr: LirRepr) -> LirValue {
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

fn fixture(opcode: OpCode, args: &[LirRepr], operands: &[u32], result: LirRepr) -> LirFunction {
    let result = value(args.len() as u32, result);
    let block = LirBlock {
        id: BlockId(0),
        args: args
            .iter()
            .enumerate()
            .map(|(i, &repr)| value(i as u32, repr))
            .collect(),
        ops: vec![LirOp {
            tir_op: TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: operands.iter().copied().map(ValueId).collect(),
                results: vec![result.id],
                attrs: AttrDict::new(),
                source_span: None,
            },
            result_values: vec![result.clone()],
        }],
        terminator: LirTerminator::Return {
            values: vec![result.id],
        },
    };
    LirFunction {
        name: format!("owned_{opcode:?}"),
        param_names: (0..args.len()).map(|i| format!("arg{i}")).collect(),
        param_types: block.args.iter().map(|v| v.ty.clone()).collect(),
        return_types: vec![result.ty],
        blocks: HashMap::from([(block.id, block)]),
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    }
}

/// Execute the same operation scope twice in one frame. Fresh function locals
/// would hide stale scratch owners left by the previous dynamic iteration.
fn loop_fixture(lazy: bool) -> LirFunction {
    let mut function = fixture(
        OpCode::Copy,
        &[LirRepr::I64, LirRepr::Bool1],
        &[0],
        LirRepr::I64,
    );
    let entry = function.blocks.get_mut(&BlockId(0)).unwrap();
    entry.ops.clear();
    entry.terminator = LirTerminator::Branch {
        target: BlockId(1),
        args: vec![ValueId(1)],
    };
    let mut operation = fixture(
        if lazy { OpCode::And } else { OpCode::BoxVal },
        &[LirRepr::I64, LirRepr::Bool1, LirRepr::Bool1],
        if lazy { &[2, 0] } else { &[0] },
        LirRepr::DynBox,
    )
    .blocks
    .remove(&BlockId(0))
    .unwrap()
    .ops;
    operation.extend([
        LirOp {
            tir_op: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::DecRef,
                operands: vec![ValueId(3)],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
            result_values: vec![],
        },
        LirOp {
            tir_op: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Not,
                operands: vec![ValueId(2)],
                results: vec![ValueId(4)],
                attrs: AttrDict::new(),
                source_span: None,
            },
            result_values: vec![value(4, LirRepr::Bool1)],
        },
    ]);
    function.blocks.insert(
        BlockId(1),
        LirBlock {
            id: BlockId(1),
            args: vec![value(2, LirRepr::Bool1)],
            ops: operation,
            terminator: LirTerminator::CondBranch {
                cond: ValueId(2),
                then_block: BlockId(1),
                then_args: vec![ValueId(4)],
                else_block: BlockId(2),
                else_args: vec![],
            },
        },
    );
    function.blocks.insert(
        BlockId(2),
        LirBlock {
            id: BlockId(2),
            args: vec![],
            ops: vec![],
            terminator: LirTerminator::Return {
                values: vec![ValueId(0)],
            },
        },
    );
    function
}

// These imports are ownership/failure observation boundaries, not substitutes
// for the runtime. Execute the actual emitted instructions on success, partial
// initialization, lazy branches, and repeated entries into the same function.
const EXECUTE: &str = r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), yes = BigInt(config.yes), wide = 1n << 60n;
let refs, children, integers, next, attempts, calls, pending, failAt, truthFail, mutationFailAt, mutations, constructorFail, unboxes;
function reset(fail = 0) {
  refs = new Map(); children = new Map(); integers = new Map(); next = 0x700000000000n; attempts = calls = mutations = unboxes = 0;
  pending = ''; failAt = fail; truthFail = constructorFail = false; mutationFailAt = 0;
}
function allocate() { const id = ++next; refs.set(id, 1); return id; }
function live(value) { assert.ok(refs.has(value), 'using dead box ' + value); }
function inc(value) { if (refs.has(value)) refs.set(value, refs.get(value) + 1); }
function dec(value) {
  if (value > 0x700000000000n && value <= next) {
    live(value); const rc = refs.get(value) - 1;
    if (rc) refs.set(value, rc);
    else {
      refs.delete(value);
      for (const child of children.get(value) || []) dec(child);
      children.delete(value);
    }
  }
}
function newContainer() {
  calls++;
  if (constructorFail) { pending = 'MemoryError'; return none; }
  const value = allocate(); children.set(value, []); return value;
}
function append(container, values) {
  live(container); values.forEach(live); calls++;
  if (++mutations === mutationFailAt) { pending = 'MutationError'; return 1; }
  for (const value of values) { inc(value); children.get(container).push(value); }
  return 0;
}
const hooks = {
  int_from_i64(value) {
    assert.ok(value >= (1n << 46n) || value < -(1n << 46n));
    if (++attempts === failAt) { pending = 'MemoryError'; return none; }
    const result = allocate(); integers.set(result, value); return result;
  },
  int_as_i64(value) { live(value); unboxes++; assert.ok(integers.has(value)); return integers.get(value); },
  inc_ref_obj: inc, dec_ref_obj: dec,
  exception_pending() { return pending ? 1n : 0n; },
  add(a, b) { live(a); live(b); calls++; return none; },
  is(a, b) { live(a); live(b); calls++; assert.equal(a, b); return yes; },
  is_truthy(value) { if (truthFail) { pending = 'TruthError'; return 0n; } return 1n; },
  dict_new(capacity) { assert.equal(capacity, 1n); return newContainer(); },
  dict_set(dict, key, value) {
    return append(dict, [key, value]) ? none : dict;
  },
  set_new(capacity) { assert.equal(capacity, BigInt(config.raw_two)); return newContainer(); },
  set_add(set, value) { append(set, [value]); return none; },
  list_builder_new(capacity) { assert.equal(capacity, BigInt(config.boxed_two)); return newContainer(); },
  list_builder_append(builder, value) { return append(builder, [value]); },
  list_builder_finish(builder) {
    live(builder); calls++; const elements = children.get(builder); children.delete(builder);
    dec(builder); const result = allocate(); children.set(result, elements); return result;
  },
  tuple_builder_finish(builder) { return hooks.list_builder_finish(builder); },
};
const loaded = {};
for (const [name, file] of Object.entries(config.modules)) {
  const run = new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(file)), {molt_runtime: hooks}).exports.run;
  loaded[name] = (...args) => {
    console.error('WASM case ' + name + '(' + args.map(String).join(', ') + ')');
    return run(...args);
  };
}
for (const failure of [0, 1, 2]) {
  reset(failure); assert.equal(loaded.add(wide, wide + 1n), none);
  assert.equal(attempts, failure === 1 ? 1 : 2);
  assert.equal(calls, failure ? 0 : 1); assert.equal(refs.size, 0);
  assert.equal(pending, failure ? 'MemoryError' : '');
}
reset(); assert.equal(loaded.same(wide), yes); assert.equal(attempts, 1); assert.equal(refs.size, 0);
for (const name of ['and', 'or', 'box', 'boxed_alias']) {
  for (const failure of [0, 1]) {
    reset(failure); const result = loaded[name](wide, wide + 1n);
    assert.equal(attempts, 1, name); assert.equal(refs.size, failure ? 0 : 1, name);
    if (!failure) { assert.equal(refs.get(result), 1, name); dec(result); }
    else assert.equal(result, none, name);
    assert.equal(refs.size, 0, name);
  }
}
reset(); loaded.and(0n, wide); assert.equal(attempts, 0); assert.equal(refs.size, 0);
reset(); truthFail = true; assert.equal(loaded.truth_fail(1n, wide), none);
assert.equal(pending, 'TruthError'); assert.equal(attempts, 0); assert.equal(refs.size, 0);
reset(); assert.equal(loaded.raw_alias(wide), wide); assert.equal(attempts, 0); assert.equal(refs.size, 0);
reset(); assert.equal(loaded.raw_rc(wide), wide); assert.equal(attempts, 0); assert.equal(refs.size, 0);
reset(); assert.equal(loaded.loop_lazy(wide, 1), wide);
assert.equal(attempts, 1); assert.equal(refs.size, 0);
for (const failure of [0, 2]) {
  reset(failure); assert.equal(loaded.loop_box(wide, 1), wide);
  assert.equal(attempts, 2); assert.equal(refs.size, 0);
  assert.equal(pending, failure ? 'MemoryError' : '');
}
for (const name of ['box_heap', 'box_ref', 'unbox_ref', 'unbox_boxed']) {
  reset(); const input = allocate(), output = loaded[name](input);
  assert.equal(output, input); assert.equal(refs.get(input), 2, name);
  dec(input); live(output); dec(output); assert.equal(refs.size, 0, name);
}
for (const failure of [0, 1]) {
  reset(failure); assert.equal(loaded.discard_box(wide), undefined);
  assert.equal(attempts, 1); assert.equal(refs.size, 0);
  assert.equal(pending, failure ? 'MemoryError' : '');
}
for (const name of ['discard_box_heap', 'discard_unbox']) {
  reset(); const input = allocate(); assert.equal(loaded[name](input), undefined);
  assert.equal(refs.get(input), 1, name); assert.equal(attempts, 0, name);
  assert.equal(unboxes, 0, name); dec(input); assert.equal(refs.size, 0, name);
}
for (const integer of [-(1n << 63n), -(1n << 46n) - 1n, 1n << 46n, (1n << 63n) - 1n]) {
  reset(); const boxed = hooks.int_from_i64(integer);
  assert.equal(loaded.unbox_int(boxed), integer); assert.equal(unboxes, 1);
  assert.equal(refs.get(boxed), 1); dec(boxed); assert.equal(refs.size, 0);
  reset(); const input = hooks.int_from_i64(integer), result = loaded.abi_roundtrip(input);
  assert.equal(integers.get(result), integer); assert.equal(unboxes, 1);
  assert.equal(refs.get(input), 1); assert.equal(refs.get(result), 1);
  dec(input); dec(result); assert.equal(refs.size, 0);
}
for (const integer of [-(1n << 46n), -1n, 0n, 1n, (1n << 46n) - 1n]) {
  reset(); const boxed = BigInt(config.int_tag) | BigInt.asUintN(47, integer);
  assert.equal(loaded.unbox_int(boxed), integer); assert.equal(unboxes, 0);
  assert.equal(loaded.abi_roundtrip(boxed), boxed); assert.equal(unboxes, 0);
}
reset(); assert.equal(loaded.unbox_bool(yes), 1); assert.equal(loaded.unbox_bool(yes ^ 1n), 0);
for (const number of [-0, 0, 1.25, -42.5, Infinity, -Infinity, NaN]) {
  const bytes = new DataView(new ArrayBuffer(8)); bytes.setFloat64(0, number);
  assert.ok(Object.is(loaded.unbox_float(bytes.getBigInt64(0)), number));
}
for (const name of ['dict', 'set', 'list', 'tuple']) {
  for (const failure of [0, 1, 2]) {
    reset(failure); const result = loaded[name](wide, wide + 1n);
    assert.equal(refs.size, failure ? 0 : 3, name);
    if (failure) { assert.equal(result, none, name); assert.equal(calls, 0, name); }
    else { assert.equal(refs.get(result), 1, name); dec(result); }
    assert.equal(refs.size, 0, name);
  }
  for (const failedMutation of (name === 'dict' ? [1] : [1, 2])) {
    reset(); mutationFailAt = failedMutation;
    assert.equal(loaded[name](wide, wide + 1n), none, name);
    assert.equal(pending, 'MutationError', name); assert.equal(refs.size, 0, name);
  }
  reset(); constructorFail = true;
  assert.equal(loaded[name](wide, wide + 1n), none, name);
  assert.equal(pending, 'MemoryError', name); assert.equal(refs.size, 0, name);
}
// Return boxing is owned by the caller, never an operation scratch lifetime.
reset(); const returned = loaded.return_box(); assert.equal(refs.get(returned), 1); dec(returned);
reset(1); assert.equal(loaded.return_box(), none); assert.equal(pending, 'MemoryError');
console.log('owned-materialization: success, partial failure, lazy arms, aliases, RC and transfers passed');
"#;

#[test]
fn emitted_materialization_scopes_execute_success_failure_and_transfers() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "LIR materialization ownership",
    )
    .expect("Node is required to prove emitted allocation-failure branches");
    let (directory, _cleanup) = wasm_test_temp_dir();
    let mut modules = serde_json::Map::new();
    for (name, opcode, args, operands) in [
        ("add", OpCode::Add, vec![LirRepr::I64; 2], vec![0, 1]),
        ("same", OpCode::Is, vec![LirRepr::I64], vec![0, 0]),
        ("and", OpCode::And, vec![LirRepr::I64; 2], vec![0, 1]),
        ("or", OpCode::Or, vec![LirRepr::I64; 2], vec![0, 1]),
        (
            "truth_fail",
            OpCode::And,
            vec![LirRepr::DynBox, LirRepr::I64],
            vec![0, 1],
        ),
        ("box", OpCode::BoxVal, vec![LirRepr::I64], vec![0]),
        ("box_heap", OpCode::BoxVal, vec![LirRepr::DynBox], vec![0]),
        ("box_ref", OpCode::BoxVal, vec![LirRepr::Ref64], vec![0]),
        (
            "unbox_boxed",
            OpCode::UnboxVal,
            vec![LirRepr::DynBox],
            vec![0],
        ),
        (
            "unbox_int",
            OpCode::UnboxVal,
            vec![LirRepr::DynBox],
            vec![0],
        ),
        (
            "unbox_bool",
            OpCode::UnboxVal,
            vec![LirRepr::DynBox],
            vec![0],
        ),
        (
            "unbox_float",
            OpCode::UnboxVal,
            vec![LirRepr::DynBox],
            vec![0],
        ),
        (
            "unbox_ref",
            OpCode::UnboxVal,
            vec![LirRepr::DynBox],
            vec![0],
        ),
        ("boxed_alias", OpCode::Copy, vec![LirRepr::I64], vec![0]),
        ("raw_alias", OpCode::Copy, vec![LirRepr::I64], vec![0]),
        ("dict", OpCode::BuildDict, vec![LirRepr::I64; 2], vec![0, 1]),
        ("set", OpCode::BuildSet, vec![LirRepr::I64; 2], vec![0, 1]),
        ("list", OpCode::BuildList, vec![LirRepr::I64; 2], vec![0, 1]),
        (
            "tuple",
            OpCode::BuildTuple,
            vec![LirRepr::I64; 2],
            vec![0, 1],
        ),
    ] {
        let repr = match name {
            "raw_alias" | "unbox_int" => LirRepr::I64,
            "unbox_bool" => LirRepr::Bool1,
            "unbox_float" => LirRepr::F64,
            "unbox_ref" => LirRepr::Ref64,
            _ => LirRepr::DynBox,
        };
        let mut function = fixture(opcode, &args, &operands, repr);
        if opcode == OpCode::Copy {
            function.blocks.get_mut(&function.entry_block).unwrap().ops[0]
                .tir_op
                .attrs
                .insert(
                    "_original_kind".into(),
                    AttrValue::Str("binding_alias".into()),
                );
        }
        let path = directory.join(format!("{name}.wasm"));
        fs::write(&path, executable_module(&lower_lir_to_wasm(&function))).unwrap();
        modules.insert(name.into(), json!(path));
    }
    for (name, opcode, repr) in [
        ("discard_box", OpCode::BoxVal, LirRepr::I64),
        ("discard_box_heap", OpCode::BoxVal, LirRepr::DynBox),
        ("discard_unbox", OpCode::UnboxVal, LirRepr::DynBox),
    ] {
        let mut function = fixture(opcode, &[repr], &[0], LirRepr::DynBox);
        function.return_types.clear();
        let block = function.blocks.get_mut(&function.entry_block).unwrap();
        block.ops[0].tir_op.results.clear();
        block.ops[0].result_values.clear();
        block.terminator = LirTerminator::Return { values: vec![] };
        let path = directory.join(format!("{name}.wasm"));
        fs::write(&path, executable_module(&lower_lir_to_wasm(&function))).unwrap();
        modules.insert(name.into(), json!(path));
    }
    let mut raw_rc = fixture(OpCode::Copy, &[LirRepr::I64], &[0], LirRepr::I64);
    let block = raw_rc.blocks.get_mut(&raw_rc.entry_block).unwrap();
    for opcode in [OpCode::IncRef, OpCode::DecRef, OpCode::DelBoundary] {
        block.ops.insert(
            0,
            LirOp {
                tir_op: TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![ValueId(0)],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
                result_values: vec![],
            },
        );
    }
    let path = directory.join("raw_rc.wasm");
    fs::write(&path, executable_module(&lower_lir_to_wasm(&raw_rc))).unwrap();
    modules.insert("raw_rc".into(), json!(path));
    let path = directory.join("return_box.wasm");
    let body = lower_tir_to_wasm_boxed_i64_abi(&make_const_return_func(1i64 << 60)).unwrap();
    fs::write(&path, executable_module(&body)).unwrap();
    modules.insert("return_box".into(), json!(path));
    let mut identity = TirFunction::new("abi_roundtrip".into(), vec![TirType::I64], TirType::I64);
    identity
        .blocks
        .get_mut(&identity.entry_block)
        .unwrap()
        .terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };
    let repr = HashMap::from([(ValueId(0), Repr::RawI64FullDeopt)]);
    let ranges = crate::representation_plan::value_range_for(&identity);
    let body =
        super::super::driver::lower_tir_to_wasm_boxed_i64_abi_with_proof(&identity, &repr, &ranges)
            .unwrap();
    let path = directory.join("abi_roundtrip.wasm");
    fs::write(&path, executable_module(&body)).unwrap();
    modules.insert("abi_roundtrip".into(), json!(path));
    for (name, lazy) in [("loop_lazy", true), ("loop_box", false)] {
        let path = directory.join(format!("{name}.wasm"));
        fs::write(
            &path,
            executable_module(&lower_lir_to_wasm(&loop_fixture(lazy))),
        )
        .unwrap();
        modules.insert(name.into(), json!(path));
    }
    let config = directory.join("cases.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "modules": modules,
            "none": box_none_bits().to_string(),
            "yes": molt_codegen_abi::box_bool_bits(1).to_string(),
            "int_tag": QNAN_TAG_INT_I64.to_string(),
            "raw_two": "2",
            "boxed_two": box_int_bits(2).to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        EXECUTE,
        &[&config],
        "execute LIR materialization ownership and failure cases",
    );
}
