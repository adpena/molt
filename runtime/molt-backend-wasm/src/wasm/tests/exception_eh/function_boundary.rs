use super::*;
use crate::ir::ExecutionContextPolicy;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

#[test]
fn python_eh_returns_preserve_guarded_calls_and_dispatch_handlers() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "Python exception ABI",
    )
    .expect("Node is required for Python exception ABI execution");
    let (temp, _cleanup) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    // Named Python handler destinations select dispatch. The plain callee
    // uses native EH; the callers must observe its canonical pending state.
    for mode in ["jumpful", "stateful"] {
        for runtime_failure in [false, true] {
            let labelled = |kind: &str| OpIR {
                value: Some(7),
                ..wasm_test_op(kind, None, vec![])
            };
            let mut ops = vec![OpIR {
                value: Some(5),
                ..wasm_test_op("trace_enter_slot", None, vec![])
            }];
            if mode == "stateful" {
                ops.push(OpIR {
                    state_targets: Some(vec![]),
                    ..wasm_test_op("state_switch", None, vec![])
                });
            }
            ops.extend([
                wasm_test_op("exception_push", None, vec![]),
                labelled("try_start"),
                wasm_test_op("binding_alias", Some("callee_obj"), vec!["incoming_callee"]),
                OpIR {
                    s_value: Some("callee".into()),
                    ..wasm_test_op("call_guarded", None, vec!["callee_obj"])
                },
                labelled("check_exception"),
                OpIR {
                    value: Some(999),
                    ..wasm_test_op("line", None, vec![])
                },
                labelled("try_end"),
            ]);
            ops.push(labelled("label"));
            ops.extend([
                wasm_test_op("dec_ref", None, vec!["callee_obj"]),
                wasm_test_op("exception_pop", None, vec![]),
                OpIR {
                    value: Some(3),
                    ..wasm_test_op("line", None, vec![])
                },
                wasm_test_op("ret_void", None, vec![]),
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
                    ..wasm_test_op("call", None, vec!["exception"])
                }
            } else {
                wasm_test_op("raise", None, vec!["exception"])
            };
            let mut callee = wasm_test_function(
                "callee",
                vec![],
                None,
                vec![
                    OpIR {
                        value: Some(1),
                        ..wasm_test_op("const_bool", Some("exception"), vec![])
                    },
                    raise,
                    wasm_test_op("ret_void", None, vec![]),
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
            .emit_wasm_module(ir, BTreeMap::new(), trampolines)
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
  let frames=0, handlers=0, recursion=0, invocations=0, owners=1, pending=false;
  const lines=[];
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
    raise: bits => { assert.equal(bits,yes); assert.equal(handlers,1); pending=true; return none; },
    function_direct_call_eligible: () => 1n,
    handle_resolve: bits => { assert.equal(bits,777n); return 128; },
    recursion_guard_enter: () => { recursion++; return 1n; },
    recursion_guard_exit: () => { assert.equal(recursion,1); recursion--; },
    frame_invocation_enter: () => { invocations++; return 42n; },
    frame_invocation_exit: token => {
      assert.equal(token,42n); assert.equal(invocations,1); invocations--; return none;
    },
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
  assert.equal(test.mode==='stateful' ? app.molt_main(64n,777n) : app.molt_main(777n),none);
  assert.deepEqual(lines,[3],test.mode+': failed call must reach its handler');
  assert.equal(pending,true);
  assert.deepEqual([frames,handlers,recursion,invocations,owners],[0,0,0,0,1],test.mode);
}
"#,
        &[&config],
        "Python exception ABI and caller guard cleanup",
    );
}
