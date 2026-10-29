use super::*;
use crate::ir::ExecutionContextPolicy;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

#[test]
fn python_eh_returns_preserve_guarded_calls_and_dispatch_handlers() {
    let node = PathBuf::from("node");
    let (temp, _cleanup) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    // Named Python handler destinations select dispatch. The plain callee
    // uses native EH; the callers must observe its canonical pending state.
    for mode in ["jumpful", "stateful"] {
        for runtime_failure in [false, true] {
            let labelled = |kind: molt_ir::OpKind| OpIR {
                value: Some(7),
                ..wasm_test_op(kind, None, vec![])
            };
            let mut ops = vec![OpIR {
                s_value: Some("molt_main".into()),
                ..wasm_test_op(molt_ir::OpKind::TraceEnterSlot, None, vec![])
            }];
            if mode == "stateful" {
                ops.push(OpIR {
                    state_targets: Some(vec![]),
                    ..wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![])
                });
            }
            ops.extend([
                wasm_test_op(molt_ir::OpKind::ExceptionPush, None, vec![]),
                labelled(molt_ir::OpKind::TryStart),
                wasm_test_op(
                    molt_ir::OpKind::BindingAlias,
                    Some("callee_obj"),
                    vec!["incoming_callee"],
                ),
                OpIR {
                    s_value: Some("callee".into()),
                    ..wasm_test_op(molt_ir::OpKind::CallGuarded, None, vec!["callee_obj"])
                },
                labelled(molt_ir::OpKind::CheckException),
                OpIR {
                    value: Some(999),
                    ..wasm_test_op(molt_ir::OpKind::Line, None, vec![])
                },
                labelled(molt_ir::OpKind::TryEnd),
            ]);
            ops.push(labelled(molt_ir::OpKind::Label));
            ops.extend([
                wasm_test_op(molt_ir::OpKind::DecRef, None, vec!["callee_obj"]),
                wasm_test_op(molt_ir::OpKind::ExceptionPop, None, vec![]),
                OpIR {
                    value: Some(3),
                    ..wasm_test_op(molt_ir::OpKind::Line, None, vec![])
                },
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
            ]);
            let mut caller = wasm_test_function(
                "molt_main",
                if mode == "stateful" {
                    vec!["task", "incoming_callee"]
                } else {
                    vec!["incoming_callee"]
                },
                None,
                ops,
            );
            caller.execution_context = ExecutionContextPolicy::Local;
            let raise = if runtime_failure {
                OpIR {
                    s_value: Some("molt_raise".into()),
                    ..wasm_test_op(molt_ir::OpKind::Call, None, vec!["exception"])
                }
            } else {
                wasm_test_op(molt_ir::OpKind::Raise, None, vec!["exception"])
            };
            let mut callee = wasm_test_function(
                "callee",
                vec![],
                None,
                vec![
                    OpIR {
                        value: Some(1),
                        ..wasm_test_op(molt_ir::OpKind::ConstBool, Some("exception"), vec![])
                    },
                    raise,
                    wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
                ],
            );
            callee.execution_context = ExecutionContextPolicy::Inherited;
            let ir = SimpleIR {
                functions: vec![caller, callee],
                profile: None,
            };
            let trampolines = crate::wasm::trampoline_analysis::analyze_wasm_trampolines(&ir);
            let wasm = WasmBackend::with_options(WasmCompileOptions {
                native_eh_enabled: true,
                reloc_enabled: false,
                ..WasmCompileOptions::default()
            })
            .with_code_slots(
                molt_ir::code_slots::CodeSlotBindings::try_from(BTreeMap::from([(
                    "molt_main".to_string(),
                    0,
                )]))
                .unwrap(),
            )
            .emit_wasm_module(
                crate::wasm::final_functions::WasmFinalFunctions::from_codegen_fixtures(ir),
                trampolines,
                crate::wasm_plan::WasmStageAudit::from_environment(),
            )
            .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
            let path = temp.join(format!("{mode}-{runtime_failure}.wasm"));
            fs::write(&path, wasm).unwrap();
            cases.push(json!({"mode":mode, "runtime_failure":runtime_failure,
                "path":path, "memory_pages":memory_pages, "table_entries":table_entries}));
        }
    }
    let config = temp.join("exception-boundaries.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"cases":cases,
        "none":molt_codegen_abi::box_none_bits().to_string(),
        "yes":molt_codegen_abi::box_bool_bits(1).to_string()}))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), yes = BigInt(config.yes);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  let frames=0, handlers=0, recursion=0, owners=1, pending=false;
  const lines=[], handed=[];
  // The thread record: the direct call writes its invocation word.
  const record=256, handoff=record+8;
  const imports = {env:{
    memory:new WebAssembly.Memory({initial:test.memory_pages}),
    __indirect_function_table:new WebAssembly.Table({initial:test.table_entries,element:'anyfunc'})
  }};
  const hooks = {
    trace_enter_slot: () => { frames++; return none; },
    trace_exit: () => { assert.equal(frames,1); frames--; return none; },
    trace_set_line: line => { assert.equal(frames,1); lines.push(Number(line)); return none; },
    obj_get_state: () => 0n,
    exception_push: () => { handlers++; return none; },
    exception_pop: () => { assert.equal(handlers,1); handlers--; return none; },
    exception_pending: () => pending ? 1n : 0n,
    raise: bits => {
      assert.equal(bits,yes); assert.equal(handlers,1); pending=true;
      handed.push(new DataView(imports.env.memory.buffer).getBigInt64(handoff,true));
      return none;
    },
    function_direct_call_eligible: () => 1n,
    handle_resolve: bits => { assert.equal(bits,777n); return 128; },
    recursion_enter_fast: () => { recursion++; return 1n; },
    recursion_exit_fast: () => { assert.equal(recursion,1); recursion--; },
    thread_record: () => BigInt(record),
    inc_ref_obj: bits => { if(bits===777n) owners++; },
    dec_ref_obj: bits => { if(bits===777n) { assert.ok(owners>0); owners--; } },
  };
  for (const entry of WebAssembly.Module.imports(module)) {
    if(entry.kind!=='function') continue;
    imports[entry.module] ??= {};
    imports[entry.module][entry.name] = hooks[entry.name] ?? (()=>{
      throw new Error(test.mode+': unexpected runtime call '+entry.name);
    });
  }
  const app = new WebAssembly.Instance(module,imports).exports;
  const table = imports.env.__indirect_function_table;
  let target = -1;
  for(let i=0;i<table.length;i++) if(table.get(i)===app.callee) { target=i; break; }
  assert.ok(target>=0,'callee must have its real table identity');
  new DataView(imports.env.memory.buffer).setBigInt64(128,BigInt(target),true);
  new DataView(imports.env.memory.buffer).setBigInt64(handoff,5n,true);
  assert.equal(test.mode==='stateful' ? app.molt_main(64n,777n) : app.molt_main(777n),none);
  assert.deepEqual(lines,[3],test.mode+': failed call must reach its handler');
  assert.equal(pending,true);
  assert.deepEqual([frames,handlers,recursion,owners],[0,0,0,1],test.mode);
  // The callee ran with itself handed over; the outer handoff came back.
  assert.deepEqual(handed,[777n],test.mode);
  assert.equal(new DataView(imports.env.memory.buffer).getBigInt64(handoff,true),5n,test.mode);
}
"#,
        &[&config],
        "Python exception ABI and caller guard cleanup",
    );
}
