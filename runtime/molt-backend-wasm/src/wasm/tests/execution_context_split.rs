use super::support::*;
use crate::ir::ExecutionContextPolicy;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
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
function reset(label, inherited = false, exceptionAt = 0, failEntry = false) {
  state = {label, depth: inherited ? 1 : 0, enters: 0, exits: 0,
           initialDepth: inherited ? 1 : 0, attempts: [], failEntry,
           lines: [], polls: 0, exceptionAt, pending: false,
           taskReads: 0, expectsTask: false, homeCase: false,
           homeLends: 0, homeOwners: new Map(), homeRetains: 0, homeReleases: 0,
           anchorCase: false, anchorFailure: 'none', anchorLive: false, anchorPtr: null,
           anchorAllocations: 0, anchorFrees: 0, anchorConstructed: 0, anchorChecks: 0,
           anchorOwners: new Map(), anchorRetains: 0, anchorReleases: 0};
}
function ensure(condition, message) {
  assert.ok(condition, state.label + ': ' + message + '; state=' + JSON.stringify(state));
}
function instantiate(path) {
  const module = new WebAssembly.Module(fs.readFileSync(path));
  const imports = {env: {
    memory: new WebAssembly.Memory({initial: config.memory_pages + Number(state.homeCase)}),
    __indirect_function_table: new WebAssembly.Table({initial: config.table_entries, element: 'anyfunc'}),
  }};
  const hooks = {
    scratch_alloc(size) {
      ensure(state.anchorCase && size===8n && state.anchorAllocations===0,
             'only A1 invocation may allocate its one anchor slot');
      ensure(state.enters===0 && state.depth===state.initialDepth,
             'anchor allocation must precede owned frame entry');
      state.anchorAllocations++;
      if (state.anchorFailure==='allocation') {state.pending=true;return 0n;}
      state.anchorPtr=config.memory_pages*65536+128;
      state.anchorLive=true;
      new Uint8Array(imports.env.memory.buffer,state.anchorPtr,8).fill(0xa5);
      return BigInt(state.anchorPtr);
    },
    scratch_free(ptr,size) {
      ensure(state.anchorCase && state.anchorLive && size===8n
             && ptr===BigInt(state.anchorPtr),'anchor region freed incorrectly');
      ensure(new DataView(imports.env.memory.buffer).getBigInt64(state.anchorPtr,true)===boxedNone,
             'anchor slot must be cleared before free');
      ensure([...state.anchorOwners.values()].every(count=>count===0),
             'anchor region freed before its owners');
      state.anchorLive=false;state.anchorFrees++;
      new Uint8Array(imports.env.memory.buffer,state.anchorPtr,8).fill(0xdd);
    },
    string_from_bytes(ptr,len,out) {
      ensure(state.anchorCase && state.anchorLive && state.anchorConstructed===0
             && state.enters===0,'unexpected anchor construction');
      const view=new DataView(imports.env.memory.buffer);
      ensure(view.getBigInt64(state.anchorPtr,true)===boxedNone,
             'poisoned anchor slot was not initialized before construction');
      assert.deepEqual(Buffer.from(new Uint8Array(imports.env.memory.buffer,Number(ptr),Number(len))),
                       Buffer.from(config.anchor_payload,'utf8'));
      state.anchorConstructed++;view.setBigInt64(out,boxedNone,true);
      if (state.anchorFailure==='constructor') {state.pending=true;return 2;}
      state.anchorOwners.set(909n,1);view.setBigInt64(out,909n,true);return 0;
    },
    frame_homes(count) {
      ensure(state.homeCase && count === 2n, 'wrong home layout request');
      ensure(state.enters === 1 && state.attempts.length === 1, 'homes lent before frame entry');
      state.homeLends++;
      if (state.failEntry || state.failLend) { state.pending = true; return 0n; }
      ensure(state.depth === state.initialDepth + 1, 'homes lent outside owned frame');
      // Two public ABI pairs in an extra host page, disjoint from application
      // data and dispatcher maps. The decoy catches wrong-slot/base reads.
      state.homeBase = config.memory_pages * 65536 + 16;
      const view = new DataView(imports.env.memory.buffer);
      for (const [slot, bits] of [[0,888n],[1,777n]]) {
        view.setBigInt64(state.homeBase + 16*slot,1n,true);
        view.setBigInt64(state.homeBase + 16*slot + 8,bits,true);
        state.homeOwners.set(bits,1);
      }
      return BigInt(state.homeBase);
    },
    obj_get_state(task) {
      ensure(state.expectsTask && task === 64n, 'unexpected task state read');
      state.taskReads++;
      return BigInt(state.resumeState ?? 0);
    },
    obj_set_state(task, resume) {
      ensure(state.expectsTask && (task & 0xffffffffn) === 64n, 'wrong suspended task');
      ensure(state.depth === state.initialDepth + 1, 'suspension outside owned frame');
      state.resumeState = Number(resume);
    },
    future_poll(future) {
      ensure(future === boxedNone && state.depth === state.initialDepth + 1,
             'await poll outside owned frame');
      return state.activationIndex === 0 ? BigInt(config.boxed_pending) : boxedNone;
    },
    handle_resolve(future) {
      ensure(future === boxedNone, 'unexpected fixture future');
      return 128;
    },
    sleep_register(task, future) {
      ensure(task === 64 && future === 128, 'wrong await registration');
      return boxedNone;
    },
    raise(exception) {
      ensure(exception === boxedTrue && state.depth === state.initialDepth + 1,
             'exception capture lost the caller frame');
      if (state.foreignException) throw state.foreignException;
      state.pending = true;
      return boxedNone;
    },
    trace_enter_slot(slot) {
      ensure(state.depth === state.initialDepth && state.enters === 0,
             'unexpected frame entry');
      ensure(slot === 0n, 'wrong owner code slot ' + slot);
      state.enters++;
      state.attempts.push(!state.failEntry);
      if (state.failEntry) {
        state.pending = true;
      } else {
        state.depth++;
      }
      return boxedNone;
    },
    trace_exit() {
      ensure(state.attempts.length === 1 && state.enters === 1 && state.exits === 0,
             'frame pop without owned entry');
      const pushed = state.attempts.pop();
      ensure(state.depth === state.initialDepth + Number(pushed),
             'frame depth does not match the owned entry attempt');
      if (state.anchorCase) ensure(!state.anchorLive && state.anchorFrees===1,
                                  'anchor region must retire before home/frame cleanup');
      if (pushed) state.depth--;
      if (state.homeCase && state.homeOwners.size) {
        // The runtime owns the homes. Exit unlinks the frame before releasing
        // bindings; compiled borrowed views must not release those bindings.
        const view = new DataView(imports.env.memory.buffer);
        for (const [slot, bits] of [[0,888n],[1,777n]]) {
          ensure(view.getBigInt64(state.homeBase+16*slot,true) === 1n
                 && view.getBigInt64(state.homeBase+16*slot+8,true) === bits,
                 'borrowed load changed its home');
          const expected = bits === 777n ? 1 + state.homeRetains : 1;
          ensure(state.homeOwners.get(bits) === expected, 'home lost or gained an owner');
          state.homeOwners.set(bits,expected-1);
          state.homeReleases++;
          view.setBigInt64(state.homeBase+16*slot,0n,true);
          view.setBigInt64(state.homeBase+16*slot+8,0n,true);
        }
      }
      state.exits++; return boxedNone;
    },
    trace_set_line(line) {
      ensure(!state.failEntry && state.depth === state.initialDepth + Number(state.enters !== 0),
             'line update outside the executing frame');
      state.lines.push(Number(line)); return boxedNone;
    },
    exception_pending() {
      if (state.anchorCase && state.enters===0) {
        ensure(state.anchorConstructed===1 && state.anchorChecks===0,
               'only constructor failure is observed before owned entry');
        state.anchorChecks++;
        return state.pending ? 1n : 0n;
      }
      ensure(state.attempts.length === 1 || (state.enters === 0 && state.depth === 1),
             'exception observer outside the owned attempt or inherited frame');
      state.polls++;
      state.pending ||= state.polls === state.exceptionAt;
      return state.pending ? 1n : 0n;
    },
    async_work_poll_and_exception_pending() {
      return hooks.exception_pending();
    },
    inc_ref_obj(value) {
      if (state.anchorCase && state.anchorOwners.has(value)) {
        ensure(value===909n && state.anchorLive && state.anchorRetains===0
               && state.anchorOwners.get(value)===1,
               'literal use must retain its selected live anchor');
        ensure(new DataView(imports.env.memory.buffer).getBigInt64(state.anchorPtr,true)===value,
               'literal use must load the private anchor slot');
        state.anchorRetains++;state.anchorOwners.set(value,2);return;
      }
      if (state.homeCase && state.homeOwners.has(value)) {
        ensure(value === 777n && state.homeRetains === 0 && state.homeOwners.get(value) === 1,
               'only the explicit borrowed return may retain a home binding');
        state.homeRetains++;
        state.homeOwners.set(value,2);
        return;
      }
      ensure(value === boxedNone || value === boxedFalse || value === boxedTrue,
             'unexpected heap retain in scalar split fixture');
    },
    dec_ref_obj(value) {
      if (state.anchorCase && state.anchorOwners.has(value)) {
        ensure(state.anchorLive && state.anchorOwners.get(value)>0,
               'literal owner released early or twice');
        const count=state.anchorOwners.get(value)-1;
        state.anchorOwners.set(value,count);state.anchorReleases++;
        if (!count) ensure(new DataView(imports.env.memory.buffer).getBigInt64(state.anchorPtr,true)===boxedNone,
                           'publish empty anchor before final release');
        return;
      }
      ensure(!state.homeCase || !state.homeOwners.has(value),
             'compiled borrowed view released its home owner');
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
function verifyOwner(exports, owner, lines, polls, exceptionAt = 0, failEntry = false) {
  // Failed capture must consume only its false attempt marker, even when a
  // caller frame already exists and would otherwise accept line mutations.
  reset(owner + ' exceptionAt=' + exceptionAt + ' failEntry=' + failEntry,
        failEntry, exceptionAt, failEntry);
  const result = exports[owner]();
  ensure(state.depth === state.initialDepth && state.enters === 1 && state.exits === 1
         && state.attempts.length === 0,
         'owner return must enter/pop exactly once');
  assert.equal(result, boxedNone, state.label + ': void payload returns boxed None');
  assert.deepEqual(state.lines, lines, state.label + ': executed chunk lines');
  assert.equal(state.polls, polls, state.label + ': executed entry/chunk exception checks');
  assert.equal(state.pending, exceptionAt !== 0 || failEntry,
               state.label + ': exception remains pending');
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
verifyOwner(app, baseline.owner, baseline.chunks.flatMap(c => c.lines), baseline.chunks.length + 1);
verifyOwner(app, baseline.owner, [], 1, 1);
verifyOwner(app, baseline.owner, [], 1, 0, true);
for (let boundary = 1; boundary <= baseline.chunks.length; boundary++) {
  verifyOwner(app, baseline.owner,
    baseline.chunks.slice(0, boundary).flatMap(c => c.lines), boundary + 1, boundary + 1);
}
// Separate status-injection fixtures cover each owner stop and fallthrough;
// they are not evidence for unmodified chunk return behavior.
for (const test of config.cases.slice(1)) {
  reset('instantiate status module ' + test.owner);
  const statusApp = instantiate(test.module);
  verifyOwner(statusApp, test.owner,
    test.chunks.slice(0, test.executed).flatMap(c => c.lines), test.executed + 1);
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
for (const test of config.return_modules) {
  reset('boxed user-ABI return ' + test.lane);
  const app = instantiate(test.module);
  state.expectsTask = test.stateful;
  const entry = app[test.export_name ?? 'molt_main'];
  const result = test.stateful ? entry(64n) : entry();
  assert.equal(result, BigInt(test.expected), state.label);
  assert.equal(state.taskReads, test.stateful ? 1 : 0, state.label);
  ensure(state.enters === 0 && state.exits === 0, 'frame-free return changed lifecycle');
}
for (const test of config.activation_modules) {
  const app = instantiate(test.module);
  let resumeState = 0;
  for (const [index, expected] of test.expected.entries()) {
    reset('owned activation ' + test.lane + ' step=' + index, true);
    state.activationIndex = index;
    state.expectsTask = test.stateful;
    state.resumeState = resumeState;
    const result = test.stateful ? app.molt_main(64n) : app.molt_main();
    assert.equal(result, BigInt(expected), state.label);
    ensure(state.depth === state.initialDepth && state.enters === 1 && state.exits === 1
           && state.attempts.length === 0, 'activation leaked or consumed caller frame');
    assert.deepEqual(state.lines, [index + 1], state.label + ': resumed body');
    resumeState = state.resumeState;
  }
}
// These are exact codegen-mode witnesses over synchronous binding homes.
// Actual generator/coroutine bindings use the distinct task-payload authority.
for (const test of config.home_modules) {
  const states = test.stateful ? [0,5] : [0];
  for (const resume of states) for (const [failure,failEntry,failLend,anchorFailure='none'] of [
    [0,false,false], [1,false,false], [2,false,false], [0,true,false], [0,false,true],
    ...(test.anchor ? [[0,false,false,'allocation'],[0,false,false,'constructor']] : []),
  ]) {
    reset('borrowed home '+test.lane+' anchor='+test.anchor+' state='+resume+' failure='+failure
          +' anchorFailure='+anchorFailure
          +' entry='+failEntry+' lend='+failLend,true,failure,failEntry);
    state.homeCase = true;
    state.anchorCase = test.anchor;
    state.anchorFailure = anchorFailure;
    state.failLend = failLend;
    state.expectsTask = test.stateful;
    state.resumeState = resume;
    const homeApp = instantiate(test.module);
    const result = test.stateful ? homeApp.molt_main(64n) : homeApp.molt_main();
    const early = anchorFailure!=='none';
    const failed = failure !== 0 || failEntry || failLend || early;
    assert.equal(result,failed ? boxedNone : 777n,state.label);
    ensure(state.depth === state.initialDepth && state.enters === Number(!early) && state.exits === Number(!early)
           && state.attempts.length === 0,'home return lost frame-attempt custody');
    assert.equal(state.homeLends,Number(!early),state.label);
    assert.equal(state.taskReads,test.stateful && !early ? 1 : 0,state.label);
    assert.equal(state.polls,early ? 0 : failEntry || failLend || failure===1 ? 1 : 2,state.label);
    assert.deepEqual(state.lines,early || failEntry || failLend || failure===1 ? [] : [resume ? 2 : 1],state.label);
    assert.equal(state.pending,failed,state.label);
    assert.equal(state.homeRetains,failed ? 0 : 1,state.label);
    assert.equal(state.homeReleases,early || failEntry || failLend ? 0 : 2,state.label);
    const used=test.anchor && !early && !failEntry && !failLend && failure!==1;
    assert.equal(state.anchorAllocations,Number(test.anchor),state.label+': A0/A1 allocation count');
    assert.equal(state.anchorFrees,Number(test.anchor && anchorFailure!=='allocation'),state.label);
    assert.equal(state.anchorConstructed,Number(test.anchor && anchorFailure!=='allocation'),state.label);
    assert.equal(state.anchorChecks,state.anchorConstructed,state.label);
    assert.equal(state.anchorRetains,Number(used),state.label);
    assert.equal(state.anchorReleases,test.anchor && !early ? 1+Number(used) : 0,state.label);
    ensure(!state.anchorLive && [...state.anchorOwners.values()].every(count=>count===0),
           'anchor storage or owner escaped home cleanup');
    if (!failed) {
      // Frame exit released the home's owner; the returned alias alone lives.
      assert.equal(state.homeOwners.get(777n),1,state.label);
      state.homeOwners.set(777n,0); // caller releases its returned owner
    }
    ensure([...state.homeOwners.values()].every(count => count === 0),
           'borrowed home or returned owner leaked');
  }
}
reset('Python exception returns through framed caller ABI', true);
const unwindApp = instantiate(config.unwind_module);
assert.equal(unwindApp.molt_main(), boxedNone);
ensure(state.pending && state.depth === state.initialDepth && state.enters === 1
       && state.exits === 1 && state.attempts.length === 0,
       'callee exception bypassed activation cleanup');
reset('foreign WASM exception retains identity through activation cleanup', true);
const foreign = new WebAssembly.Exception(new WebAssembly.Tag({parameters: []}), []);
state.foreignException = foreign;
assert.throws(() => unwindApp.molt_main(), error => error === foreign);
ensure(!state.pending && state.depth === state.initialDepth && state.enters === 1
       && state.exits === 1, 'foreign exception bypassed activation cleanup');
console.log('split-frame execution: checked/failed entry, actual chunks, normal/exceptional owner, status edges, negative controls passed');
"#;

fn split_frame_fixture(name: &str) -> (FunctionIR, Vec<FunctionIR>) {
    let mut ops = vec![
        OpIR {
            kind: "trace_enter_slot".into(),
            s_value: Some(name.into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(0),
            ..OpIR::default()
        },
    ];
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
        OpIR {
            kind: "label".into(),
            value: Some(0),
            ..OpIR::default()
        },
        wasm_test_op("trace_exit", None, vec![]),
        wasm_test_op("ret_void", None, vec![]),
    ]);
    let original = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
                // Keep the observer's admitted owner; each control changes
                // only frame lifecycle, never code identity.
                body.instruction(&Instruction::I64Const(0));
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
    // The optimized LIR lane must preserve lifecycle operations or decline the
    // function through its existing typed unsupported-operation admission.
    let mut local_fast_candidate = wasm_test_function(
        "probe____molt_globals_builtin__owned",
        vec![],
        None,
        vec![
            OpIR {
                kind: "trace_enter_slot".into(),
                s_value: Some("probe____molt_globals_builtin__owned".into()),
                ..OpIR::default()
            },
            wasm_test_op("trace_exit", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    local_fast_candidate.execution_context = ExecutionContextPolicy::Local;
    let candidate_name = local_fast_candidate.name.clone();
    let plans = crate::wasm::final_functions::WasmFinalFunctions::lowering_plans_for_fixtures(
        &SimpleIR {
            functions: vec![local_fast_candidate],
            profile: None,
        },
        &BTreeSet::new(),
    );
    assert!(
        matches!(
            plans[&candidate_name],
            crate::wasm::lir_fast::WasmFunctionLoweringPlan::Generic { .. }
        ),
        "the LIR lane must not erase Python frame ownership"
    );
    let node = PathBuf::from("node");
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
        .with_code_slots(
            molt_ir::code_slots::CodeSlotBindings::try_from(BTreeMap::from([(
                case["owner"].as_str().unwrap().to_string(),
                0,
            )]))
            .unwrap(),
        )
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
    let mut return_modules = Vec::new();
    for mode in ["ordinary", "dispatch", "stateful"] {
        for payload in [false, true] {
            let mut ops = Vec::new();
            if mode == "dispatch" {
                ops.extend(["jump", "label"].map(|kind| OpIR {
                    kind: kind.into(),
                    value: Some(7),
                    ..OpIR::default()
                }));
            } else if mode == "stateful" {
                ops.push(OpIR {
                    state_targets: Some(vec![]),
                    ..wasm_test_op("state_switch", None, vec![])
                });
            }
            if payload {
                ops.push(OpIR {
                    kind: "const_bool".into(),
                    value: Some(1),
                    out: Some("result".into()),
                    ..OpIR::default()
                });
                ops.push(wasm_test_op("ret", None, vec!["result"]));
            } else {
                ops.push(wasm_test_op("ret_void", None, vec![]));
            }
            let params = if mode == "stateful" {
                vec!["task"]
            } else {
                vec![]
            };
            let function = wasm_test_function("molt_main", params, None, ops);
            assert_eq!(function.return_abi, molt_ir::FunctionReturnAbi::Value);
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(
                SimpleIR {
                    functions: vec![function],
                    profile: None,
                },
                Default::default(),
            );
            let (pages, entries) = wasm_import_minimums(&output.wasm);
            memory_pages = memory_pages.max(pages);
            table_entries = table_entries.max(entries);
            let lane = format!("{}-{}", mode, if payload { "payload" } else { "empty" });
            let module = temp.join(format!("return_{lane}.wasm"));
            fs::write(&module, output.wasm).expect("write return ABI module");
            let expected = if payload {
                molt_codegen_abi::box_bool_bits(1)
            } else {
                molt_codegen_abi::box_none_bits()
            };
            return_modules
                .push(json!({"lane": lane, "module": module, "expected": expected.to_string(), "stateful": mode == "stateful"}));
        }
    }
    // Exercise the public compiler and actual name-selected plan consumer.
    // The existing return observer supplies the executable boxed-value oracle;
    // private plan tests separately establish admission and exact rejection.
    for return_abi in [
        molt_ir::FunctionReturnAbi::Value,
        molt_ir::FunctionReturnAbi::Void,
    ] {
        let returns_value = return_abi == molt_ir::FunctionReturnAbi::Value;
        let export_name = "molt_test_return_abi____molt_globals_builtin__";
        let wasm = WasmBackend::new().compile(SimpleIR {
            functions: vec![FunctionIR {
                name: export_name.into(),
                return_abi,
                ops: vec![
                    OpIR {
                        kind: "const".into(),
                        value: Some(42),
                        out: Some("result".into()),
                        ..Default::default()
                    },
                    wasm_test_op(
                        if returns_value { "ret" } else { "ret_void" },
                        None,
                        if returns_value {
                            vec!["result"]
                        } else {
                            vec![]
                        },
                    ),
                ],
                ..Default::default()
            }],
            profile: None,
        });
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let (pages, entries) = wasm_import_minimums(&wasm);
        memory_pages = memory_pages.max(pages);
        table_entries = table_entries.max(entries);
        let lane = if returns_value {
            "proven-const-value"
        } else {
            "authored-void"
        };
        let module = temp.join(format!("return_{lane}.wasm"));
        fs::write(&module, wasm).expect("write public compiler return module");
        let expected = if returns_value {
            molt_codegen_abi::box_int_bits(42)
        } else {
            molt_codegen_abi::box_none_bits()
        };
        return_modules.push(
            json!({"lane": lane, "module": module, "export_name": export_name,
            "expected": expected.to_string(), "stateful": false}),
        );
    }
    let mut activation_modules = Vec::new();
    for mode in ["ordinary", "dispatch", "stateful", "yield", "await"] {
        let stateful = matches!(mode, "stateful" | "yield" | "await");
        let mut ops = vec![OpIR {
            kind: "trace_enter_slot".into(),
            s_value: Some("molt_main".into()),
            ..OpIR::default()
        }];
        if stateful {
            ops.push(OpIR {
                state_targets: (mode == "stateful").then(Vec::new),
                ..wasm_test_op("state_switch", None, vec![])
            });
        } else if mode == "dispatch" {
            ops.extend(["jump", "label"].map(|kind| OpIR {
                kind: kind.into(),
                value: Some(7),
                ..OpIR::default()
            }));
        }
        ops.push(OpIR {
            kind: "line".into(),
            value: Some(1),
            ..OpIR::default()
        });
        let mut expected = Vec::new();
        if mode == "yield" {
            ops.extend([
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(1),
                    out: Some("yielded".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_yield".into(),
                    value: Some(1),
                    args: Some(vec!["yielded".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(2),
                    ..OpIR::default()
                },
            ]);
            expected.push(molt_codegen_abi::box_bool_bits(1).to_string());
        }
        if mode == "await" {
            ops.extend([
                OpIR {
                    kind: "state_label".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                wasm_test_op("const_none", Some("future"), vec![]),
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("resume".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_transition".into(),
                    value: Some(1),
                    args: Some(vec!["future".into(), "resume".into()]),
                    out: Some("awaited".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(2),
                    ..OpIR::default()
                },
            ]);
            expected.push(molt_codegen_abi::box_pending_bits().to_string());
        }
        ops.extend([
            wasm_test_op("trace_exit", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ]);
        expected.push(molt_codegen_abi::box_none_bits().to_string());
        let mut function = wasm_test_function(
            "molt_main",
            if stateful { vec!["task"] } else { vec![] },
            None,
            ops,
        );
        function.execution_context = ExecutionContextPolicy::Local;
        let ir = SimpleIR {
            functions: vec![function],
            profile: None,
        };
        crate::validate_simple_ir(&ir).unwrap();
        let output = if matches!(mode, "yield" | "await") {
            wasm_compile_activation_fixture(
                ir,
                molt_ir::code_slots::CodeSlotBindings::try_from(BTreeMap::from([(
                    "molt_main".to_string(),
                    0,
                )]))
                .unwrap(),
            )
        } else {
            wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(
                ir,
                molt_ir::code_slots::CodeSlotBindings::try_from(BTreeMap::from([(
                    "molt_main".to_string(),
                    0,
                )]))
                .unwrap(),
            )
        };
        wasmparser::Validator::new()
            .validate_all(&output.wasm)
            .unwrap();
        let (pages, entries) = wasm_import_minimums(&output.wasm);
        memory_pages = memory_pages.max(pages);
        table_entries = table_entries.max(entries);
        let module = temp.join(format!("activation_{mode}.wasm"));
        fs::write(&module, output.wasm).expect("write owned activation module");
        activation_modules.push(
            json!({"lane": mode, "module": module, "expected": expected, "stateful": stateful}),
        );
    }
    let mut home_modules = Vec::new();
    for (stateful, anchor) in [(false, false), (true, false), (false, true), (true, true)] {
        let marked = |kind: &str, value| OpIR {
            value: Some(value),
            ..wasm_test_op(kind, None, vec![])
        };
        let mut ops = vec![
            OpIR {
                s_value: Some("molt_main".into()),
                ..wasm_test_op("trace_enter_slot", None, vec![])
            },
            marked("check_exception", 90),
        ];
        if stateful {
            ops.push(OpIR {
                state_targets: Some(vec![(5, 50)]),
                ..wasm_test_op("state_switch", None, vec![])
            });
        } else {
            ops.push(marked("label", 0));
        }
        for (line, result) in [(1, "initial_home"), (2, "resumed_home")] {
            if line == 2 {
                if !stateful {
                    break;
                }
                ops.push(marked("state_label", 50));
            }
            ops.push(marked("line", line));
            if anchor {
                ops.push(OpIR {
                    s_value: Some("home-anchor-β\0雪".into()),
                    ..wasm_test_op("const_str", Some("unused_anchor"), vec![])
                });
            }
            ops.extend([
                OpIR {
                    value: Some(1),
                    ..wasm_test_op("frame_home_load", Some(result), vec![])
                },
                marked("check_exception", 90),
                // Final ownership IR makes the escaping result an owner. The
                // borrowed load itself must emit no retain or release.
                wasm_test_op("inc_ref", None, vec![result]),
                wasm_test_op("trace_exit", None, vec![]),
                wasm_test_op("ret", None, vec![result]),
            ]);
        }
        ops.extend([
            marked("label", 90),
            wasm_test_op("trace_exit", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ]);
        let mut function = wasm_test_function(
            "molt_main",
            if stateful { vec!["task"] } else { vec![] },
            None,
            ops,
        );
        function.execution_context = ExecutionContextPolicy::Local;
        let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(
            SimpleIR {
                functions: vec![function],
                profile: None,
            },
            molt_ir::code_slots::CodeSlotBindings::try_from(BTreeMap::from([(
                "molt_main".into(),
                0,
            )]))
            .unwrap(),
        );
        wasmparser::Validator::new()
            .validate_all(&output.wasm)
            .unwrap();
        if !stateful {
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            assert!(operators.iter().all(|op| !op.starts_with("BrTable")));
            assert_eq!(
                operators.iter().filter(|op| op.starts_with("Loop")).count(),
                if anchor { 2 } else { 0 },
                "only anchor initialization/cleanup loops in forward-only control"
            );
        }
        let (pages, entries) = wasm_import_minimums(&output.wasm);
        memory_pages = memory_pages.max(pages);
        table_entries = table_entries.max(entries);
        let lane = if stateful { "stateful" } else { "forward" };
        let module = temp.join(format!("borrowed_home_{lane}_{anchor}.wasm"));
        fs::write(&module, output.wasm).expect("write borrowed home module");
        home_modules.push(json!({"lane":lane,"module":module,"stateful":stateful,"anchor":anchor}));
    }
    // A Python throw from an inherited callee returns pending state across
    // the call ABI, so the caller retires its activation and call guards.
    let mut unwind_owner = wasm_test_function(
        "molt_main",
        vec![],
        None,
        vec![
            OpIR {
                kind: "trace_enter_slot".into(),
                s_value: Some("molt_main".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_internal".into(),
                s_value: Some("throwing_callee".into()),
                args: Some(vec![]),
                passes_execution_context: true,
                ..OpIR::default()
            },
            wasm_test_op("trace_exit", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    unwind_owner.execution_context = ExecutionContextPolicy::Local;
    let mut throwing_callee = wasm_test_function(
        "throwing_callee",
        vec![],
        None,
        vec![
            OpIR {
                kind: "const_bool".into(),
                value: Some(1),
                out: Some("exception".into()),
                ..OpIR::default()
            },
            wasm_test_op("raise", None, vec!["exception"]),
            wasm_test_op("unreachable", None, vec![]),
        ],
    );
    throwing_callee.execution_context = ExecutionContextPolicy::Inherited;
    let unwind_ir = SimpleIR {
        functions: vec![unwind_owner, throwing_callee],
        profile: None,
    };
    crate::validate_simple_ir(&unwind_ir).unwrap();
    let trampolines = crate::wasm::trampoline_analysis::analyze_wasm_trampolines(&unwind_ir);
    let unwind = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: true,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
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
        crate::wasm::final_functions::WasmFinalFunctions::from_codegen_fixtures(unwind_ir),
        trampolines,
        crate::wasm_plan::WasmStageAudit::from_environment(),
    );
    wasmparser::Validator::new()
        .validate_all(&unwind.wasm)
        .unwrap();
    let (pages, entries) = wasm_import_minimums(&unwind.wasm);
    memory_pages = memory_pages.max(pages);
    table_entries = table_entries.max(entries);
    let unwind_path = temp.join("unwind_frame.wasm");
    fs::write(&unwind_path, unwind.wasm).expect("write frame unwind module");
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
            "boxed_pending": molt_codegen_abi::box_pending_bits().to_string(),
            "anchor_payload": "home-anchor-β\0雪",
            "cases": cases,
            "return_modules": return_modules,
            "activation_modules": activation_modules,
            "home_modules": home_modules,
            "unwind_module": unwind_path,
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

#[test]
fn symbolic_code_slots_emit_program_bound_immediates() {
    let mut function = wasm_test_function(
        "molt_main",
        vec!["code", "globals"],
        None,
        vec![
            OpIR {
                s_value: Some("molt_main".into()),
                ..wasm_test_op("code_slot_set", None, vec!["code", "globals"])
            },
            OpIR {
                s_value: Some("molt_main".into()),
                ..wasm_test_op("trace_enter_slot", None, vec![])
            },
            wasm_test_op("trace_exit", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    function.execution_context = ExecutionContextPolicy::Local;
    let ir = SimpleIR {
        functions: vec![function],
        profile: None,
    };
    let error = WasmBackend::new()
        .compile_checked(ir.clone())
        .expect_err("an owner must have a program binding before emission");
    assert!(matches!(
        error.shape.violation,
        molt_ir::ir_schema::OpShapeViolation::UnboundCodeSlot
    ));
    for expected_slot in [0, 1] {
        let mut bindings = BTreeMap::from([("molt_main".to_string(), expected_slot)]);
        if expected_slot == 1 {
            bindings.insert("earlier_module".to_string(), 0);
        }
        let bindings = molt_ir::code_slots::CodeSlotBindings::try_from(bindings).unwrap();
        bindings.validate_functions(&ir.functions).unwrap();
        let wasm =
            wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir.clone(), bindings).wasm;
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let imports = wasm_function_import_indices(&wasm);
        let operators = wasm_operator_debug_for_export(&wasm, "molt_main");
        for (import, argument_distance) in [("code_slot_set", 3), ("trace_enter_slot", 1)] {
            let call = format!("Call {{ function_index: {} }}", imports[import]);
            let index = operators.iter().position(|op| *op == call).unwrap();
            assert_eq!(
                operators[index - argument_distance],
                format!("I64Const {{ value: {expected_slot} }}"),
                "{import} must use the final program's immediate slot binding"
            );
        }
    }
}

#[test]
fn symbolic_module_slots_bind_in_generic_and_lir_final_emission() {
    for name in ["module_slot_probe", "demo____molt_globals_builtin__"] {
        let function = wasm_test_function(
            name,
            vec![],
            None,
            vec![
                OpIR {
                    s_value: Some("sys".into()),
                    ..wasm_test_op("module_slot", Some("slot"), vec![])
                },
                wasm_test_op("ret", None, vec!["slot"]),
            ],
        );
        let main = wasm_test_function(
            "molt_main",
            vec![],
            None,
            vec![wasm_test_op("ret_void", None, vec![])],
        );
        let ir = SimpleIR {
            functions: vec![main, function],
            profile: None,
        };
        let error = WasmBackend::new().compile_checked(ir.clone()).unwrap_err();
        assert_eq!(
            error.shape.violation,
            molt_ir::ir_schema::OpShapeViolation::UnboundModuleSlot
        );
        let plans = crate::wasm::final_functions::WasmFinalFunctions::lowering_plans_for_fixtures(
            &ir,
            &Default::default(),
        );
        let lir = name.contains("____molt_globals_builtin__");
        if lir {
            assert!(
                plans[name].lir_fast_body().is_some(),
                "module binding must remain on the LIR path"
            );
        }
        for (names, expected) in [(vec!["sys"], 0), (vec!["added", "sys"], 1)] {
            let registry = molt_ir::ModuleRegistryIR {
                blob: vec![0],
                // The real catalog root keeps the inspected body independently
                // of inlining and ordinary call-graph reachability.
                init_symbols: vec![name.into()],
                rows: names
                    .into_iter()
                    .map(|module| molt_ir::ModuleRegistryRowIR {
                        name: module.into(),
                        init_symbol: (module == "sys").then(|| name.into()),
                    })
                    .collect(),
                ..Default::default()
            };
            let wasm = WasmBackend::new()
                .with_module_registry(Some(registry))
                .compile_checked(ir.clone())
                .unwrap()
                .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let operators = wasm_operator_debug_for_export(&wasm, name);
            let value = if lir {
                expected
            } else {
                molt_codegen_abi::box_int_bits(expected)
            };
            assert!(
                operators.contains(&format!("I64Const {{ value: {value} }}")),
                "{name}: {operators:?}"
            );
        }
    }
}
