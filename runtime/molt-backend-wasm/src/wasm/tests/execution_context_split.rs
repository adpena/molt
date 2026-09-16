use super::support::*;
use crate::ir::ExecutionContextPolicy;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

// Execute real dispatch conditions: a value-oblivious CFG walker invents
// impossible state-dispatch paths before entry. Runtime imports are explicit
// observation boundaries, never generic zero stubs.
const EXECUTE_FRAME_CASES: &str = r#"
const fs = require('fs');
const assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const boxedNone = BigInt(config.boxed_none);
const boxedFalse = BigInt(config.boxed_false);
const boxedTrue = BigInt(config.boxed_true);
let state;
function reset(label, inherited = false, exceptionAt = 0) {
  state = {label, depth: inherited ? 1 : 0, enters: 0, exits: 0,
           lines: [], polls: 0, exceptionAt, pending: false};
}
function ensure(condition, message) {
  assert.ok(condition, state.label + ': ' + message + '; state=' + JSON.stringify(state));
}
function instantiate(path) {
  const module = new WebAssembly.Module(fs.readFileSync(path));
  const imports = {env: {
    memory: new WebAssembly.Memory({initial: config.memory_pages}),
    __indirect_function_table: new WebAssembly.Table({initial: config.table_entries, element: 'anyfunc'}),
  }};
  const hooks = {
    trace_enter_slot(slot) {
      ensure(state.depth === 0 && state.enters === 0, 'unexpected frame entry');
      ensure(slot === 5n, 'wrong owner code slot ' + slot);
      state.depth++; state.enters++; return boxedNone;
    },
    trace_exit() {
      ensure(state.depth === 1 && state.enters === 1 && state.exits === 0,
             'frame pop without owned entry');
      state.depth--; state.exits++; return boxedNone;
    },
    trace_set_line(line) {
      ensure(state.depth === 1, 'line update outside the executing frame');
      state.lines.push(Number(line)); return boxedNone;
    },
    exception_pending() {
      ensure(state.depth === 1, 'exception observer outside the executing frame');
      state.polls++;
      state.pending ||= state.polls === state.exceptionAt;
      return state.pending ? 1n : 0n;
    },
    async_work_poll_and_exception_pending() {
      return hooks.exception_pending();
    },
    inc_ref_obj(value) {
      ensure(value === boxedNone || value === boxedFalse || value === boxedTrue,
             'unexpected heap retain in scalar split fixture');
    },
    dec_ref_obj(value) {
      ensure(value === boxedNone || value === boxedFalse || value === boxedTrue,
             'unexpected heap release in scalar split fixture');
    },
  };
  for (const entry of WebAssembly.Module.imports(module)) {
    if (entry.module === 'env' && entry.kind !== 'function') {
      assert.ok(entry.name in imports.env, 'unexpected host surface ' + entry.name);
      continue;
    }
    assert.equal(entry.kind, 'function', 'unexpected import ' + entry.module + '.' + entry.name);
    imports[entry.module] ??= {};
    imports[entry.module][entry.name] = hooks[entry.name] ?? (() => {
      throw new Error(state.label + ': unexpected runtime call ' + entry.module + '.' + entry.name);
    });
  }
  return new WebAssembly.Instance(module, imports).exports;
}
function verifyOwner(exports, owner, lines, polls, exceptionAt = 0) {
  reset(owner + ' exceptionAt=' + exceptionAt, false, exceptionAt);
  exports[owner]();
  ensure(state.depth === 0 && state.enters === 1 && state.exits === 1,
         'owner return must enter/pop exactly once');
  assert.deepEqual(state.lines, lines, state.label + ': executed chunk lines');
  assert.equal(state.polls, polls, state.label + ': executed chunk exception checks');
  assert.equal(state.pending, exceptionAt !== 0, state.label + ': exception remains pending');
}
reset('instantiate split module');
const baseline = config.cases[0];
const app = instantiate(baseline.module);
// Unmodified generated chunks: their real status and inherited frame behavior.
for (const chunk of baseline.chunks) {
  reset('actual inherited chunk ' + chunk.name, true);
  const status = app[chunk.name]();
  assert.equal(status, chunk.continues ? boxedTrue : boxedFalse, state.label);
  ensure(state.depth === 1 && state.enters === 0 && state.exits === 0,
         'inherited chunk changed caller frame ownership');
  assert.deepEqual(state.lines, chunk.lines, state.label);
  assert.equal(state.polls, 0, state.label);
}
// Actual owner/chunks: normal return and pending exception at each boundary.
verifyOwner(app, baseline.owner, baseline.chunks.flatMap(c => c.lines), baseline.chunks.length);
for (let boundary = 1; boundary <= baseline.chunks.length; boundary++) {
  verifyOwner(app, baseline.owner,
    baseline.chunks.slice(0, boundary).flatMap(c => c.lines), boundary, boundary);
}
// Separate status-injection fixtures cover each owner stop and fallthrough;
// they are not evidence for unmodified chunk return behavior.
for (const test of config.cases.slice(1)) {
  reset('instantiate status module ' + test.owner);
  const statusApp = instantiate(test.module);
  verifyOwner(statusApp, test.owner,
    test.chunks.slice(0, test.executed).flatMap(c => c.lines), test.executed);
}
// The same observer rejects executable malformed frame lifecycles.
reset('instantiate malformed-frame controls');
const malformed = instantiate(config.negative_module);
for (const [name, message] of [
  ['missing_entry', /frame pop without owned entry/],
  ['double_pop', /frame pop without owned entry/],
  ['reentry', /unexpected frame entry/],
  ['missing_exit', /owner return must enter\/pop exactly once/],
]) {
  assert.throws(() => verifyOwner(malformed, name, [], 0), message, name);
}
console.log('split-frame execution: actual chunks, normal/exceptional owner, status edges, negative controls passed');
"#;

fn split_frame_fixture(name: &str) -> (FunctionIR, Vec<FunctionIR>) {
    let mut ops = vec![OpIR {
        kind: "trace_enter_slot".into(),
        value: Some(5),
        ..OpIR::default()
    }];
    for line in 1..=6 {
        ops.push(OpIR {
            kind: "line".into(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".into(),
            out: Some(format!("v{line}")),
            ..OpIR::default()
        });
    }
    ops.extend([
        wasm_test_op("trace_exit", None, vec![]),
        wasm_test_op("ret_void", None, vec![]),
    ]);
    let original = FunctionIR {
        name: name.into(),
        ops,
        execution_context: ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    };
    let mut occupied = BTreeSet::from([original.name.clone()]);
    crate::passes::split_large_function(original, 3, &mut occupied).unwrap()
}

fn chunk_case(chunk: &FunctionIR) -> serde_json::Value {
    assert_eq!(chunk.execution_context, ExecutionContextPolicy::Inherited);
    let returned = chunk.ops.iter().find(|op| op.kind == "ret").unwrap();
    let returned_name = &returned.args.as_ref().unwrap()[0];
    let producer = chunk
        .ops
        .iter()
        .find(|op| op.out.as_ref() == Some(returned_name))
        .unwrap();
    assert_eq!(producer.kind, "const_bool");
    json!({
        "name": chunk.name,
        "continues": producer.value == Some(1),
        "lines": chunk.ops.iter().filter(|op| op.kind == "line")
            .map(|op| op.value.unwrap()).collect::<Vec<_>>(),
    })
}

fn malformed_frame_module() -> Vec<u8> {
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
        ImportSection, Instruction, Module, TypeSection, ValType,
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], [ValType::I64]);
    types.ty().function([], [ValType::I64]);
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("molt_runtime", "trace_enter_slot", EntityType::Function(0));
    imports.import("molt_runtime", "trace_exit", EntityType::Function(1));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    for (index, (name, calls)) in [
        ("missing_entry", vec![1]),
        ("double_pop", vec![0, 1, 1]),
        ("reentry", vec![0, 0, 1]),
        ("missing_exit", vec![0]),
    ]
    .into_iter()
    .enumerate()
    {
        functions.function(2);
        exports.export(name, ExportKind::Func, 2 + index as u32);
        let mut body = Function::new([]);
        for callee in calls {
            if callee == 0 {
                body.instruction(&Instruction::I64Const(5));
            }
            body.instruction(&Instruction::Call(callee));
            body.instruction(&Instruction::Drop);
        }
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
    module.finish()
}

#[test]
fn wasm_compiles_split_local_frame_with_inherited_chunks() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "split-frame execution proof",
    )
    .expect("Node is required: a static dispatch walker cannot prove emitted frame paths");
    let (stub, chunks) = split_frame_fixture("wasm_framed_large");
    assert!(
        chunks.len() > 1,
        "fixture must exercise cross-chunk ownership"
    );
    let chunk_count = chunks.len();
    let baseline = json!({
        "owner": stub.name,
        "chunks": chunks.iter().map(chunk_case).collect::<Vec<_>>(),
    });
    let mut fixtures = vec![(
        baseline,
        std::iter::once(stub).chain(chunks).collect::<Vec<_>>(),
    )];
    // Preserve the actual fixture. These sibling functions explicitly inject
    // only the chunk status contract to execute every owner exit.
    for stop_at in 0..=chunk_count {
        let (stub, mut chunks) = split_frame_fixture(&format!("wasm_framed_status_{stop_at}"));
        assert_eq!(chunks.len(), chunk_count);
        for (index, chunk) in chunks.iter_mut().enumerate() {
            for op in &mut chunk.ops {
                if op.kind == "const_bool"
                    && op
                        .out
                        .as_deref()
                        .is_some_and(|name| name.starts_with("__molt_split_continue_"))
                {
                    op.value = Some(i64::from(index != stop_at));
                }
            }
        }
        let case = json!({
            "owner": stub.name,
            "chunks": chunks.iter().map(chunk_case).collect::<Vec<_>>(),
            "executed": (stop_at + 1).min(chunk_count),
        });
        fixtures.push((
            case,
            std::iter::once(stub).chain(chunks).collect::<Vec<_>>(),
        ));
    }
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    let mut memory_pages = 0;
    let mut table_entries = 0;
    for (index, (mut case, functions)) in fixtures.into_iter().enumerate() {
        // Each scenario owns a real module entry. Merely placing sibling owners
        // in the baseline module does not root them: the production pipeline
        // correctly removes those unreferenced functions and all their chunks.
        let ir = SimpleIR {
            functions,
            profile: None,
        };
        crate::validate_simple_ir(&ir).unwrap();
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(ir);
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let import_indices = wasm_function_import_indices(&wasm);
        let enter = import_indices["trace_enter_slot"];
        let exit = import_indices["trace_exit"];
        let exports = wasm_function_export_indices(&wasm);
        let owner = case["owner"].as_str().unwrap();
        assert!(exports.contains_key(owner), "missing split owner {owner}");
        for chunk in case["chunks"].as_array().unwrap() {
            let name = chunk["name"].as_str().unwrap();
            if !exports.contains_key(name) {
                // Actual baseline chunks are executed independently below and
                // must exist. Injected-status fixtures prove owner behavior;
                // their unreachable chunks may be optimized away legitimately.
                assert_ne!(index, 0, "missing original split chunk {name}");
                continue;
            }
            let calls = wasm_direct_call_indices_for_export(&wasm, name);
            assert!(
                !calls.contains(&enter),
                "inherited chunk {name} minted a frame"
            );
            assert!(
                !calls.contains(&exit),
                "inherited chunk {name} popped its caller"
            );
        }
        for payload in Parser::new(0).parse_all(&wasm) {
            if let Payload::ImportSection(reader) = payload.unwrap() {
                for import in reader.into_imports() {
                    match import.unwrap().ty {
                        TypeRef::Memory(ty) => memory_pages = memory_pages.max(ty.initial),
                        TypeRef::Table(ty) => table_entries = table_entries.max(ty.initial),
                        _ => {}
                    }
                }
            }
        }
        let module_path = temp.join(format!("split_frame_{index}.wasm"));
        fs::write(&module_path, wasm).expect("write emitted split frame module");
        case["module"] = json!(module_path);
        cases.push(case);
    }
    let negative = malformed_frame_module();
    wasmparser::Validator::new()
        .validate_all(&negative)
        .unwrap();
    let negative_path = temp.join("malformed_frame.wasm");
    let config_path = temp.join("split_frame_cases.json");
    fs::write(&negative_path, negative).expect("write executable frame negative controls");
    fs::write(
        &config_path,
        serde_json::to_vec(&json!({
            "negative_module": negative_path,
            "memory_pages": memory_pages,
            "table_entries": table_entries,
            "boxed_none": molt_codegen_abi::box_none_bits().to_string(),
            "boxed_false": molt_codegen_abi::box_bool_bits(0).to_string(),
            "boxed_true": molt_codegen_abi::box_bool_bits(1).to_string(),
            "cases": cases,
        }))
        .unwrap(),
    )
    .expect("write named split frame execution cases");
    run_node_test_script(
        &node,
        EXECUTE_FRAME_CASES,
        &[&config_path],
        &format!(
            "execute emitted frame ownership cases from {}",
            config_path.display()
        ),
    );
}

#[test]
fn exported_body_inspection_counts_import_entries_not_unique_names() {
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
        ImportSection, Instruction, Module, TypeSection,
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("first", "same_name", EntityType::Function(0));
    imports.import("second", "same_name", EntityType::Function(0));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("owner", ExportKind::Func, 2);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(1));
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    let wasm = module.finish();
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    assert_eq!(wasm_direct_call_indices_for_export(&wasm, "owner"), vec![1]);
    assert_eq!(wasm_operator_debug_for_export(&wasm, "owner").len(), 2);
}
