use super::super::lir_context::LirLowerCtx;
use super::super::lir_runtime_ops::emit_lir_runtime_result;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::execution_support::executable_module;
use super::*;
use crate::tir::blocks::BlockId;
use crate::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use crate::wasm::body::WasmBody;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

fn value(id: u32, repr: LirRepr) -> LirValue {
    LirValue {
        id: ValueId(id),
        repr,
        ty: match repr {
            LirRepr::I64 => TirType::I64,
            LirRepr::F64 => TirType::F64,
            LirRepr::Bool1 => TirType::Bool,
            _ => TirType::DynBox,
        },
    }
}

fn machine_type(repr: LirRepr) -> ValType {
    match repr {
        LirRepr::F64 => ValType::F64,
        LirRepr::Bool1 => ValType::I32,
        _ => ValType::I64,
    }
}

/// Exercise the production typed sink with the same operation-owner scope as
/// the LIR driver, including a raw operand whose temporary box is returned.
fn sink_body(
    call: LirRuntimeCall,
    args: &[LirRepr],
    result: Option<LirRepr>,
    box_args: bool,
) -> WasmBody {
    let output = result.map(|repr| value(args.len() as u32, repr));
    let op = LirOp {
        tir_op: TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: (0..args.len()).map(|index| ValueId(index as u32)).collect(),
            results: output.as_ref().map(|v| v.id).into_iter().collect(),
            attrs: AttrDict::new(),
            source_span: None,
        },
        result_values: output.clone().into_iter().collect(),
    };
    let block = LirBlock {
        id: BlockId(0),
        args: args
            .iter()
            .enumerate()
            .map(|(index, &repr)| value(index as u32, repr))
            .collect(),
        ops: vec![op.clone()],
        terminator: LirTerminator::Return {
            values: output.as_ref().map(|v| v.id).into_iter().collect(),
        },
    };
    let function = LirFunction {
        name: "runtime_return_sink".into(),
        param_names: (0..args.len()).map(|index| format!("arg{index}")).collect(),
        param_types: block.args.iter().map(|v| v.ty.clone()).collect(),
        return_types: output.as_ref().map(|v| v.ty.clone()).into_iter().collect(),
        blocks: HashMap::from([(block.id, block)]),
        entry_block: BlockId(0),
        label_id_map: HashMap::new(),
    };
    let mut ctx = LirLowerCtx::new_with_local_base(&function, 0);
    ctx.allocate_function_locals();
    ctx.begin_operation_owners();
    for &operand in &op.tir_op.operands {
        if box_args {
            emit_get_boxed_for_repr(&mut ctx, operand);
        } else {
            ctx.emit_get(operand);
        }
    }
    ctx.emit_runtime_call(call);
    emit_lir_runtime_result(&mut ctx, &op, call);
    ctx.finish_operation_owners(&op);
    if let Some(output) = output {
        ctx.emit_get(output.id);
    }
    ctx.instructions.push(Instruction::End);
    WasmBody {
        param_types: args.iter().copied().map(machine_type).collect(),
        result_types: result.map(machine_type).into_iter().collect(),
        locals: ctx.local_declarations_after(args.len() as u32),
        ops: ctx.instructions.into_vec(),
    }
}

#[test]
fn lir_import_result_sink_rejects_width_and_lifetime_mismatches() {
    for (call, result) in [
        (LirRuntimeCall::IntAsI64, LirRepr::DynBox),
        (LirRuntimeCall::HandleResolve, LirRepr::I64),
        (LirRuntimeCall::ModuleNew, LirRepr::I64),
        (LirRuntimeCall::ModuleNew, LirRepr::F64),
    ] {
        assert_eq!(
            sink_body(call, &[LirRepr::DynBox], Some(result), false).bail_to_generic_reason(),
            Some(WasmLirFallbackReason::UnsupportedOperation)
        );
    }
    for (call, result) in [
        (LirRuntimeCall::DecRefObj, Some(LirRepr::DynBox)),
        (LirRuntimeCall::Alloc, None),
        (LirRuntimeCall::Alloc, Some(LirRepr::DynBox)),
        (LirRuntimeCall::ScratchAlloc, None),
    ] {
        assert!(
            std::panic::catch_unwind(|| sink_body(call, &[LirRepr::I64], result, false)).is_err()
        );
    }
    // The ordinary preserved-Copy path must also retain the exact call token
    // through its no-result sink, not merely pass a synthetic sink test.
    let function = make_copy_original_kind_runtime_func("discard_module", "module_new", 1, false);
    let body = lower_tir_to_wasm(&function);
    assert_eq!(body.bail_to_generic_reason(), None);
    assert!(body.test_view().runtime_calls.contains(&"dec_ref_obj"));
    executable_module(&body);
}

#[test]
fn lir_import_result_contracts_execute_owned_borrowed_raw_and_void_paths() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "LIR runtime return ownership",
    )
    .expect("Node is required to prove emitted runtime result ownership");
    let (directory, _cleanup) = wasm_test_temp_dir();
    let mut modules = serde_json::Map::new();
    for (name, call, args, result, box_args) in [
        (
            "owned_bound",
            LirRuntimeCall::ModuleNew,
            vec![LirRepr::DynBox],
            Some(LirRepr::DynBox),
            false,
        ),
        (
            "owned_discard",
            LirRuntimeCall::ModuleNew,
            vec![LirRepr::DynBox],
            None,
            false,
        ),
        (
            "borrowed_bound",
            LirRuntimeCall::DictSet,
            vec![LirRepr::DynBox; 3],
            Some(LirRepr::DynBox),
            false,
        ),
        (
            "borrowed_discard",
            LirRuntimeCall::DictSet,
            vec![LirRepr::DynBox; 3],
            None,
            false,
        ),
        (
            "borrowed_temporary",
            LirRuntimeCall::DictSet,
            vec![LirRepr::I64, LirRepr::DynBox, LirRepr::DynBox],
            Some(LirRepr::DynBox),
            true,
        ),
        (
            "raw_i64",
            LirRuntimeCall::IntAsI64,
            vec![LirRepr::DynBox],
            Some(LirRepr::I64),
            false,
        ),
        (
            "raw_i32",
            LirRuntimeCall::HandleResolve,
            vec![LirRepr::DynBox],
            Some(LirRepr::Bool1),
            false,
        ),
        (
            "raw_discard",
            LirRuntimeCall::IntAsI64,
            vec![LirRepr::DynBox],
            None,
            false,
        ),
        (
            "void",
            LirRuntimeCall::DecRefObj,
            vec![LirRepr::DynBox],
            None,
            false,
        ),
        (
            "boxed_bool",
            LirRuntimeCall::Eq,
            vec![LirRepr::DynBox; 2],
            Some(LirRepr::Bool1),
            false,
        ),
    ] {
        let body = sink_body(call, &args, result, box_args);
        assert_eq!(body.bail_to_generic_reason(), None, "{name}");
        if name == "boxed_bool" {
            assert_eq!(body.test_view().runtime_calls, vec!["eq", "dec_ref_obj"]);
        }
        let path = directory.join(format!("{name}.wasm"));
        fs::write(&path, executable_module(&body)).unwrap();
        modules.insert(name.into(), json!(path));
    }
    let config = directory.join("cases.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "modules": modules, "none": box_none_bits().to_string(),
            "yes": molt_codegen_abi::box_bool_bits(1).to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        EXECUTE,
        &[&config],
        "execute typed LIR runtime result ownership",
    );
}

// These providers model return credits at the actual emitted call boundaries;
// they are not implementations of dictionary/module semantics.
const EXECUTE: &str = r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), yes = BigInt(config.yes);
let owners = new Map(), next = 1000n, allocated = 0, retained = 0, released = 0;
function fresh() { const value = ++next; owners.set(value, 1); allocated++; return value; }
function retain(value) {
  if (!owners.has(value)) return;
  assert(owners.get(value) > 0, 'retain after final release');
  owners.set(value, owners.get(value) + 1); retained++;
}
function release(value) {
  if (!owners.has(value)) return;
  assert(owners.get(value) > 0, 'duplicate release');
  owners.set(value, owners.get(value) - 1); released++;
}
function counts(a, i, d, live) {
  assert.deepEqual([allocated, retained, released, [...owners.values()].reduce((a,b)=>a+b,0)], [a,i,d,live]);
}
function reset() {
  assert.equal([...owners.values()].reduce((a,b)=>a+b,0), 0, 'leaked owner');
  owners = new Map(); allocated = retained = released = 0;
}
const host = {module_new:fresh, dict_set:(value)=>value, int_from_i64:fresh,
  int_as_i64:()=>72n, handle_resolve:()=>1, inc_ref_obj:retain, dec_ref_obj:release, eq:()=>yes};
function run(name, ...args) {
  const module = new WebAssembly.Module(fs.readFileSync(config.modules[name]));
  return new WebAssembly.Instance(module, {molt_runtime:host}).exports.run(...args);
}
let value = run('owned_bound', none); counts(1,0,0,1); release(value); counts(1,0,1,0); reset();
run('owned_discard', none); counts(1,0,1,0); reset();
let source = fresh(); value = run('borrowed_bound', source, none, none);
assert.equal(value, source); counts(1,1,0,2); release(value);
run('borrowed_discard', source, none, none); counts(1,1,1,1); release(source); reset();
value = run('borrowed_temporary', 1n << 60n, none, none);
counts(1,1,1,1); release(value); counts(1,1,2,0); reset();
assert.equal(run('raw_i64', none), 72n); assert.equal(run('raw_i32', none), 1);
run('raw_discard', none); counts(0,0,0,0); reset();
source = fresh(); run('void', source); counts(1,0,1,0); reset();
assert.equal(run('boxed_bool', none, none), 1); counts(0,0,0,0); reset();
"#;
