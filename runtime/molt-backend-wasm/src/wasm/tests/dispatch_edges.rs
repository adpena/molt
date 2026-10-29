use super::support::*;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};
use wasmparser::Operator;

fn marked(kind: molt_ir::OpKind, value: i64) -> OpIR {
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

// This checks the property that changes engine tiering, not a textual source
// shape: an acyclic guest must not contain branches to a WASM loop.
fn direct_loop_edges(wasm: &[u8]) -> usize {
    let mut count = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            let mut labels = vec![false];
            for op in body.get_operators_reader().unwrap() {
                match op.unwrap() {
                    Operator::Loop { .. } => labels.push(true),
                    Operator::Block { .. } | Operator::If { .. } | Operator::TryTable { .. } => {
                        labels.push(false)
                    }
                    Operator::End => {
                        labels.pop();
                    }
                    Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => {
                        count += usize::from(labels[labels.len() - 1 - relative_depth as usize]);
                    }
                    Operator::BrTable { targets } => {
                        for depth in targets
                            .targets()
                            .chain(std::iter::once(Ok(targets.default())))
                        {
                            count +=
                                usize::from(labels[labels.len() - 1 - depth.unwrap() as usize]);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    count
}

// Inspect encoded branch alternatives rather than the planner under test.
fn dispatcher_target_counts(wasm: &[u8]) -> Vec<usize> {
    let mut counts = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            for op in body.get_operators_reader().unwrap() {
                if let Operator::BrTable { targets } = op.unwrap() {
                    counts.push(targets.len() as usize);
                }
            }
        }
    }
    counts
}

#[test]
#[should_panic(expected = "stateful fallthrough requires an explicit terminal return")]
fn dispatch_edges_reject_stateful_implicit_return() {
    emit(wasm_test_function(
        "molt_main",
        vec!["task"],
        None,
        vec![
            OpIR {
                state_targets: Some(vec![]),
                ..wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![])
            },
            marked(molt_ir::OpKind::Line, 1),
        ],
    ));
}

fn saved_state_guest(state_id: i64) -> Vec<u8> {
    emit(wasm_test_function(
        "molt_main",
        vec!["task"],
        None,
        vec![
            OpIR {
                state_targets: Some(vec![(state_id, state_id)]),
                ..wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![])
            },
            marked(molt_ir::OpKind::Line, 10),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
            marked(molt_ir::OpKind::StateLabel, state_id),
            marked(molt_ir::OpKind::Line, 20),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
        ],
    ))
}

fn saved_state_roundtrip_guest(state_id: i64, anchored: bool) -> Vec<u8> {
    emit(wasm_test_function(
        "molt_main",
        vec!["task"],
        None,
        vec![
            marked(molt_ir::OpKind::Line, 8),
            marked(molt_ir::OpKind::CheckException, 90),
            OpIR {
                state_targets: Some(vec![(state_id, 70)]),
                ..wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![])
            },
            marked(molt_ir::OpKind::Line, 10),
            marked(molt_ir::OpKind::CheckException, 90),
            marked(molt_ir::OpKind::StateSet, state_id),
            OpIR {
                value: Some(0),
                ..wasm_test_op(molt_ir::OpKind::ConstBool, Some("initial"), vec![])
            },
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["initial"]),
            // Authored operation 8 is the resume identity. Its persisted word
            // is ~8, independently of physical blocks or dispatcher ordinals.
            marked(molt_ir::OpKind::StateLabel, 70),
            marked(molt_ir::OpKind::Line, 20),
            marked(molt_ir::OpKind::CheckException, 90),
            OpIR {
                value: (!anchored).then_some(1),
                s_value: anchored.then(|| "poll-anchor-β\0雪".to_string()),
                ..wasm_test_op(
                    if anchored {
                        molt_ir::OpKind::ConstStr
                    } else {
                        molt_ir::OpKind::ConstBool
                    },
                    Some("resumed"),
                    vec![],
                )
            },
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["resumed"]),
            marked(molt_ir::OpKind::Label, 90),
            marked(molt_ir::OpKind::Line, 90),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
        ],
    ))
}

#[test]
fn dispatch_edges_preserve_paths_without_synthetic_forward_backedges() {
    let node = PathBuf::from("node");
    let (temp, _cleanup) = wasm_test_temp_dir();
    let mut linear = vec![marked(molt_ir::OpKind::Label, 0)];
    for line in 1..=64 {
        linear.push(marked(molt_ir::OpKind::Line, line));
        linear.push(marked(molt_ir::OpKind::CheckException, 90));
    }
    linear.push(OpIR {
        args: Some(vec!["condition".into()]),
        ..marked(molt_ir::OpKind::BrIf, 80)
    });
    linear.extend([
        OpIR {
            value: Some(0),
            ..wasm_test_op(molt_ir::OpKind::ConstBool, Some("no"), vec![])
        },
        wasm_test_op(molt_ir::OpKind::Ret, None, vec!["no"]),
        marked(molt_ir::OpKind::Label, 80),
        OpIR {
            value: Some(1),
            ..wasm_test_op(molt_ir::OpKind::ConstBool, Some("yes"), vec![])
        },
        wasm_test_op(molt_ir::OpKind::Ret, None, vec!["yes"]),
        marked(molt_ir::OpKind::Label, 90),
        wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
    ]);
    let linear = emit(wasm_test_function(
        "molt_main",
        vec!["condition"],
        None,
        linear,
    ));
    assert_eq!(
        direct_loop_edges(&linear),
        0,
        "acyclic exception/branch edges became loop backedges"
    );

    assert!(
        dispatcher_target_counts(&linear).is_empty(),
        "forward DAG must have no dynamic alternatives"
    );

    let looping = emit(wasm_test_function(
        "molt_main",
        vec![],
        None,
        vec![
            marked(molt_ir::OpKind::Label, 0),
            wasm_test_op(molt_ir::OpKind::LoopStart, None, vec![]),
            marked(molt_ir::OpKind::Line, 7),
            wasm_test_op(molt_ir::OpKind::LoopBreakIfException, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopContinue, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopEnd, None, vec![]),
            OpIR {
                value: Some(1),
                ..wasm_test_op(molt_ir::OpKind::ConstBool, Some("yes"), vec![])
            },
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["yes"]),
        ],
    ));
    assert_eq!(
        direct_loop_edges(&looping),
        1,
        "only the actual continue may redispatch"
    );

    assert_eq!(
        dispatcher_target_counts(&looping),
        vec![2],
        "only invocation entry and the real loop header may dispatch"
    );
    let latch_guest = |jumpful| {
        let mut ops = Vec::new();
        if jumpful {
            ops.push(marked(molt_ir::OpKind::Label, 0));
        }
        ops.extend([
            wasm_test_op(molt_ir::OpKind::LoopStart, None, vec![]),
            marked(molt_ir::OpKind::Line, 7),
            wasm_test_op(molt_ir::OpKind::LoopBreakIfException, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopEnd, None, vec![]),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
        ]);
        emit(wasm_test_function("molt_main", vec![], None, ops))
    };
    let latch_plain = latch_guest(false);
    let latch_jumpful = latch_guest(true);
    assert_eq!(direct_loop_edges(&latch_plain), 1);
    assert_eq!(direct_loop_edges(&latch_jumpful), 1);
    assert_eq!(dispatcher_target_counts(&latch_jumpful), vec![2]);

    let latch_stateful = emit(wasm_test_function(
        "molt_main",
        vec!["task"],
        None,
        vec![
            OpIR {
                state_targets: Some(vec![]),
                ..wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![])
            },
            wasm_test_op(molt_ir::OpKind::LoopStart, None, vec![]),
            marked(molt_ir::OpKind::Line, 7),
            wasm_test_op(molt_ir::OpKind::LoopBreakIfException, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopEnd, None, vec![]),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
        ],
    ));
    assert_eq!(
        direct_loop_edges(&latch_stateful),
        2,
        "saved-state entry and reachable closer both retain their canonical transfers"
    );

    let implicit = emit(wasm_test_function(
        "molt_main",
        vec![],
        None,
        vec![
            marked(molt_ir::OpKind::Label, 0),
            marked(molt_ir::OpKind::Line, 99),
        ],
    ));
    assert_eq!(
        direct_loop_edges(&implicit),
        0,
        "implicit return must exit activation"
    );
    let ready = wasm_compile_activation_fixture(
        SimpleIR {
            functions: vec![wasm_test_function(
                "molt_main",
                vec!["task"],
                None,
                vec![
                    wasm_test_op(molt_ir::OpKind::StateSwitch, None, vec![]),
                    marked(molt_ir::OpKind::StateLabel, 1),
                    wasm_test_op(molt_ir::OpKind::ConstNone, Some("future"), vec![]),
                    OpIR {
                        value: Some(1),
                        ..wasm_test_op(molt_ir::OpKind::Const, Some("resume"), vec![])
                    },
                    OpIR {
                        value: Some(1),
                        ..wasm_test_op(
                            molt_ir::OpKind::StateTransition,
                            Some("awaited"),
                            vec!["future", "resume"],
                        )
                    },
                    marked(molt_ir::OpKind::Line, 23),
                    wasm_test_op(molt_ir::OpKind::Ret, None, vec!["awaited"]),
                ],
            )],
            profile: None,
        },
        Default::default(),
    )
    .wasm;
    assert_eq!(
        direct_loop_edges(&ready),
        1,
        "only saved-state entry may redispatch"
    );
    let structured = emit(wasm_test_function(
        "molt_main",
        vec!["condition"],
        None,
        vec![
            marked(molt_ir::OpKind::Label, 0),
            wasm_test_op(molt_ir::OpKind::If, None, vec!["condition"]),
            marked(molt_ir::OpKind::Line, 1),
            wasm_test_op(molt_ir::OpKind::Else, None, vec![]),
            marked(molt_ir::OpKind::Line, 2),
            wasm_test_op(molt_ir::OpKind::EndIf, None, vec![]),
            marked(molt_ir::OpKind::Line, 3),
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["condition"]),
        ],
    ));
    assert_eq!(direct_loop_edges(&structured), 0);
    assert!(dispatcher_target_counts(&structured).is_empty());
    assert!(dispatcher_target_counts(&implicit).is_empty());
    let marker_loop = emit(wasm_test_function(
        "molt_main",
        vec!["condition"],
        None,
        vec![
            marked(molt_ir::OpKind::Label, 0),
            wasm_test_op(molt_ir::OpKind::If, None, vec!["condition"]),
            marked(molt_ir::OpKind::Line, 8),
            wasm_test_op(molt_ir::OpKind::EndIf, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopStart, None, vec![]),
            marked(molt_ir::OpKind::Line, 7),
            wasm_test_op(molt_ir::OpKind::LoopBreakIfException, None, vec![]),
            wasm_test_op(molt_ir::OpKind::LoopEnd, None, vec![]),
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["condition"]),
        ],
    ));
    assert_eq!(dispatcher_target_counts(&marker_loop), vec![2]);
    let marker_exit = emit(wasm_test_function(
        "molt_main",
        vec!["condition"],
        None,
        vec![
            marked(molt_ir::OpKind::Label, 0),
            wasm_test_op(molt_ir::OpKind::If, None, vec!["condition"]),
            marked(molt_ir::OpKind::Line, 1),
            wasm_test_op(molt_ir::OpKind::EndIf, None, vec![]),
        ],
    ));
    assert!(dispatcher_target_counts(&marker_exit).is_empty());

    let backward = emit(wasm_test_function(
        "molt_main",
        vec!["condition"],
        None,
        vec![
            marked(molt_ir::OpKind::Jump, 20),
            marked(molt_ir::OpKind::Label, 10),
            marked(molt_ir::OpKind::Line, 10),
            wasm_test_op(molt_ir::OpKind::Ret, None, vec!["condition"]),
            marked(molt_ir::OpKind::Label, 20),
            marked(molt_ir::OpKind::Line, 20),
            marked(molt_ir::OpKind::CheckException, 10),
            OpIR {
                args: Some(vec!["condition".into()]),
                ..marked(molt_ir::OpKind::BrIf, 10)
            },
            marked(molt_ir::OpKind::Line, 30),
            marked(molt_ir::OpKind::Jump, 10),
        ],
    ));
    assert_eq!(direct_loop_edges(&backward), 3);
    let final_check = emit(wasm_test_function(
        "molt_main",
        vec![],
        None,
        vec![
            marked(molt_ir::OpKind::Jump, 20),
            marked(molt_ir::OpKind::Label, 10),
            marked(molt_ir::OpKind::Line, 10),
            wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
            marked(molt_ir::OpKind::Label, 20),
            marked(molt_ir::OpKind::Line, 20),
            marked(molt_ir::OpKind::CheckException, 10),
        ],
    ));
    assert_eq!(direct_loop_edges(&final_check), 1);
    let mut cases = Vec::new();
    for (name, wasm) in [
        ("linear", linear),
        ("looping", looping),
        ("latch_plain", latch_plain),
        ("latch_jumpful", latch_jumpful),
        ("latch_stateful", latch_stateful),
        ("implicit", implicit),
        ("ready", ready),
        ("structured", structured),
        ("marker_loop", marker_loop),
        ("marker_exit", marker_exit),
        ("backward", backward),
        ("final_check", final_check),
        ("saved_dense", saved_state_guest(3)),
        ("saved_sparse", saved_state_guest(10000)),
        ("roundtrip_dense", saved_state_roundtrip_guest(3, false)),
        (
            "roundtrip_sparse",
            saved_state_roundtrip_guest(10000, false),
        ),
        (
            "roundtrip_anchor_dense",
            saved_state_roundtrip_guest(3, true),
        ),
        (
            "roundtrip_anchor_sparse",
            saved_state_roundtrip_guest(10000, true),
        ),
    ] {
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
        let path = temp.join(format!("{name}.wasm"));
        fs::write(&path, &wasm).unwrap();
        cases.push(json!({"name":name,"path":path,"memory_pages":memory_pages,"table_entries":table_entries}));
    }
    let config = temp.join("dispatch.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"cases":cases,
            "none":molt_codegen_abi::box_none_bits().to_string(),
            "yes":molt_codegen_abi::box_bool_bits(1).to_string(),
            "no":molt_codegen_abi::box_bool_bits(0).to_string(),
            "anchor_payload":"poll-anchor-β\0雪",
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), yes = BigInt(config.yes), no = BigInt(config.no);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  const anchored = test.name.startsWith('roundtrip_anchor_');
  let lines, polls, failAt, savedState=0n;
  let stateReads=0, stateWrites=[];
  let anchor, nextOwner=1001n;
  const owners=new Map(), returnedOwners=new Set();
  const hooks = {
    trace_set_line(line) { lines.push(Number(line)); return none; },
    exception_pending() {
      if (anchored && !anchor.checked) {
        assert.equal(anchor.constructed,1,'constructor observer precedes poll prefix');
        anchor.checked=true;
        return anchor.pending ? 1n : 0n;
      }
      return ++polls === failAt ? 1n : 0n;
    },
    is_truthy(value) { assert.ok(value === yes || value === no); return value === yes ? 1n : 0n; },
    is_truthy_int(value) { return hooks.is_truthy(value); },
    inc_ref_obj(value) {
      if (anchored && owners.has(value)) {
        assert.equal(value,anchor.owner,'resumed use selects this poll\'s payload owner');
        assert.equal(anchor.live,true); assert.equal(anchor.uses,0);
        assert.equal(lines.at(-1),20,'heap use belongs only to resumed body');
        assert.equal(new DataView(imports.env.memory.buffer).getBigInt64(anchor.ptr,true),value);
        assert.equal(owners.get(value),1); owners.set(value,2); anchor.uses++;
        return;
      }
      assert.ok([none,yes,no].includes(value));
    },
    dec_ref_obj(value) {
      if (anchored && owners.has(value)) {
        assert.equal(value,anchor.owner); assert.equal(anchor.live,true);
        assert.equal(new DataView(imports.env.memory.buffer).getBigInt64(anchor.ptr,true),none,
                     'clear this poll\'s slot before anchor release');
        assert.ok(owners.get(value)>0); owners.set(value,owners.get(value)-1);
        anchor.releases++; return;
      }
      assert.ok([none,yes,no].includes(value));
    },
    scratch_alloc(size) {
      assert.equal(anchored,true,'A0 poll must not allocate');
      assert.equal(size,8n); assert.equal(anchor.allocations++,0);
      assert.equal(anchor.live,false);
      if (anchor.scenario==='allocation') {anchor.pending=true;return 0n;}
      anchor.ptr=test.memory_pages*65536+128; anchor.live=true;
      new Uint8Array(imports.env.memory.buffer,anchor.ptr,8).fill(0xa5);
      return BigInt(anchor.ptr);
    },
    scratch_free(ptr,size) {
      assert.equal(anchored,true);assert.equal(anchor.live,true);
      assert.equal(ptr,BigInt(anchor.ptr));assert.equal(size,8n);
      assert.equal(new DataView(imports.env.memory.buffer).getBigInt64(anchor.ptr,true),none);
      if (anchor.owner!==null) assert.equal(owners.get(anchor.owner),anchor.uses,
        'region free preserves exactly the escaped literal owner');
      anchor.live=false;anchor.frees++;
      new Uint8Array(imports.env.memory.buffer,anchor.ptr,8).fill(0xdd);
    },
    string_from_bytes(ptr,len,out) {
      assert.equal(anchored,true);assert.equal(anchor.live,true);
      assert.equal(anchor.constructed++,0);
      const view=new DataView(imports.env.memory.buffer);
      assert.equal(view.getBigInt64(anchor.ptr,true),none,'poisoned slot initialized before constructor');
      assert.deepEqual(Buffer.from(new Uint8Array(imports.env.memory.buffer,Number(ptr),Number(len))),
                       Buffer.from(config.anchor_payload,'utf8'));
      view.setBigInt64(out,none,true);
      if (anchor.scenario==='constructor') {anchor.pending=true;return 2;}
      anchor.owner=nextOwner++;owners.set(anchor.owner,1);
      view.setBigInt64(out,anchor.owner,true);return 0;
    },
    obj_get_state(task) { assert.equal(task, 64n); stateReads++; return savedState; },
    obj_set_state(task, state) {
      assert.equal(task & 0xffffffffn, 64n);
      if (test.name.startsWith('roundtrip_')) {
        assert.equal(task,64n,'compiled poll retains the raw task parameter');
        stateWrites.push(state); savedState=state;
      }
    },
    future_poll(future) { assert.equal(future, none); return yes; },
  };
  const imports = {env:{memory:new WebAssembly.Memory({initial:test.memory_pages+Number(anchored)}),
    __indirect_function_table:new WebAssembly.Table({initial:test.table_entries,element:'anyfunc'})}};
  for (const entry of WebAssembly.Module.imports(module)) {
    if (entry.kind !== 'function') { assert.ok(entry.name in imports.env); continue; }
    imports[entry.module] ??= {};
    imports[entry.module][entry.name] = hooks[entry.name] ?? (() => {throw Error('unexpected import '+entry.name);});
  }
  const app = new WebAssembly.Instance(module, imports).exports;
  if (test.name === 'linear') {
    for (const failure of [0,1,32,64]) for (const condition of [no,yes]) {
      lines=[]; polls=0; failAt=failure;
      assert.equal(app.molt_main(condition), failure ? none : condition);
      assert.deepEqual(lines, Array.from({length:failure || 64}, (_,i)=>i+1));
      assert.equal(polls, failure || 64);
    }
    // This forward-only body has no operation map to corrupt. Arbitrary
    // lookup-data bytes cannot invent an entry alternative or skip observers.
    new Uint8Array(imports.env.memory.buffer).fill(255);
    lines=[]; polls=0; failAt=0;
    assert.equal(app.molt_main(yes), yes);
    assert.deepEqual(lines, Array.from({length:64}, (_,i)=>i+1));
    assert.equal(polls,64);
  } else if (test.name === 'looping') {
    lines=[]; polls=0; failAt=3;
    assert.equal(app.molt_main(), yes);
    assert.deepEqual(lines, [7,7,7]); assert.equal(polls, 3);
    // Dynamic lookup remains a checked boundary: corrupt ordinals trap before
    // any callback, rather than selecting an unplanned body or loop backedge.
    new Uint8Array(imports.env.memory.buffer).fill(255);
    lines=[]; polls=0;
    assert.throws(()=>app.molt_main(), WebAssembly.RuntimeError);
    assert.deepEqual(lines, []); assert.equal(polls,0);
  } else if (test.name === 'latch_plain' || test.name === 'latch_jumpful') {
    lines=[]; polls=0; failAt=3;
    assert.equal(app.molt_main(), none);
    assert.deepEqual(lines,[7,7,7]); assert.equal(polls,3);
  } else if (test.name === 'latch_stateful') {
    // Raw negative operation4 still selects its existing containing block;
    // the formerly incorrect closer fallthrough now takes the canonical latch.
    for (const state of [0n,~1n,~4n]) {
      savedState=state; lines=[]; polls=0; failAt=3;
      assert.equal(app.molt_main(64n),none);
      assert.deepEqual(lines,[7,7,7]); assert.equal(polls,3);
    }
  } else if (test.name === 'ready') {
    lines=[]; polls=0; failAt=0;
    assert.equal(app.molt_main(64n), yes); assert.deepEqual(lines, [23]);
    for (const invalid of [2n, 0x100000000n, -999n]) {
      savedState=invalid; lines=[];
      assert.throws(()=>app.molt_main(64n), WebAssembly.RuntimeError);
      assert.deepEqual(lines, []);
    }
  } else if (test.name === 'structured') {
    for (const condition of [no,yes]) {
      lines=[];
      assert.equal(app.molt_main(condition), condition);
      assert.deepEqual(lines, [condition===yes?1:2,3]);
    }
  } else if (test.name === 'marker_loop') {
    for (const condition of [no,yes]) {
      lines=[]; polls=0; failAt=3;
      assert.equal(app.molt_main(condition),condition);
      assert.deepEqual(lines,condition===yes?[8,7,7,7]:[7,7,7]);
      assert.equal(polls,3);
    }
  } else if (test.name === 'marker_exit') {
    for (const condition of [no,yes]) {
      lines=[]; polls=0; failAt=0;
      assert.equal(app.molt_main(condition),none);
      assert.deepEqual(lines,condition===yes?[1]:[]);
    }
  } else if (test.name.startsWith('roundtrip_')) {
    const resume = test.name.endsWith('dense') ? 3n : 10000n;
    // No planner/map helper supplies this expectation: the authored source
    // continuation is operation 8, so the external saved word must be ~8.
    const encoded = ~8n;
    const reset = (failure=0,scenario='normal') => {
      lines=[]; polls=0; failAt=failure; stateReads=0; stateWrites=[];
      anchor={scenario,owner:null,ptr:null,live:false,pending:false,checked:false,
              allocations:0,frees:0,constructed:0,uses:0,releases:0};
    };
    const finishAnchor = (result,uses) => {
      assert.equal(anchor.allocations,Number(anchored),'one allocation per actual poll, none for A0');
      assert.equal(anchor.constructed,Number(anchored && anchor.scenario!=='allocation'));
      assert.equal(anchor.frees,Number(anchored && anchor.scenario!=='allocation'));
      assert.equal(anchor.live,false);assert.equal(anchor.uses,uses);
      assert.equal(anchor.releases,Number(anchor.owner!==null));
      if (anchor.owner!==null) {
        assert.equal(owners.get(anchor.owner),uses);
        if (uses) {assert.equal(result,anchor.owner);returnedOwners.add(result);}
      }
      for (const owner of returnedOwners) assert.equal(owners.get(owner),1,
        'later poll cannot retire an earlier returned owner');
    };
    const releaseReturned = (owner) => {
      assert.equal(anchor.live,false);assert.equal(returnedOwners.delete(owner),true);
      assert.equal(owners.get(owner),1);owners.set(owner,0);
    };
    savedState=0n; reset();
    assert.equal(app.molt_main(64n), no);
    finishAnchor(no,0);
    assert.deepEqual(lines,[8,10]); assert.equal(polls,2);
    assert.equal(stateReads,1); assert.deepEqual(stateWrites,[encoded]);
    assert.equal(savedState,encoded);
    // Feed the actual persisted runtime word back into the next poll. Entry
    // prefix runs once again; initial body and state_set must not replay.
    reset();
    const resumed=app.molt_main(64n);
    assert.equal(resumed,anchored ? anchor.owner : yes);
    finishAnchor(resumed,Number(anchored));
    assert.deepEqual(lines,[8,20]); assert.equal(polls,2);
    assert.equal(stateReads,1); assert.deepEqual(stateWrites,[]);
    assert.equal(savedState,encoded);
    // Positive public state IDs and persisted operation identities share the
    // same real continuation, for both dense and sparse remap plans.
    savedState=resume; reset();
    const positive=app.molt_main(64n);
    assert.equal(positive,anchored ? anchor.owner : yes);
    finishAnchor(positive,Number(anchored));
    if (anchored) {assert.notEqual(resumed,positive);releaseReturned(resumed);releaseReturned(positive);}
    assert.deepEqual(lines,[8,20]); assert.equal(polls,2);
    assert.equal(stateReads,1); assert.deepEqual(stateWrites,[]);
    assert.equal(savedState,resume);
    // An exception in either the prefix or selected body reaches only the
    // authored handler and cannot publish a state or replay another body.
    for (const state of [0n,encoded]) for (const failure of [1,2]) {
      savedState=state; reset(failure);
      assert.equal(app.molt_main(64n),none);
      finishAnchor(none,0);
      assert.deepEqual(lines,failure===1 ? [8,90] : [8,state===0n?10:20,90]);
      assert.equal(polls,failure); assert.equal(stateReads,1);
      assert.deepEqual(stateWrites,[]); assert.equal(savedState,state);
    }
    if (anchored) for (const scenario of ['allocation','constructor']) {
      savedState=encoded;reset(0,scenario);
      assert.equal(app.molt_main(64n),none);finishAnchor(none,0);
      assert.equal(anchor.pending,true);
      assert.deepEqual(lines,[]);assert.equal(polls,0);assert.equal(stateReads,0);
      assert.deepEqual(stateWrites,[]);assert.equal(savedState,encoded);
    }
    assert.equal(returnedOwners.size,0);
    assert.ok([...owners.values()].every(count=>count===0));
  } else if (test.name.startsWith('saved_')) {
    const resume = test.name==='saved_dense' ? 3n : 10000n;
    for (const [state, expected] of [[0n,10],[resume,20]]) {
      savedState=state; lines=[];
      assert.equal(app.molt_main(64n), none);
      assert.deepEqual(lines, [expected]);
    }
    for (const invalid of [1n,2n,resume-1n,resume+1n,0x100000000n]) {
      savedState=invalid; lines=[];
      assert.throws(()=>app.molt_main(64n), WebAssembly.RuntimeError);
      assert.deepEqual(lines, []);
    }
  } else if (test.name === 'backward') {
    for (const condition of [no,yes]) for (const failure of [0,1]) {
      lines=[]; polls=0; failAt=failure;
      assert.equal(app.molt_main(condition), condition);
      assert.deepEqual(lines, failure || condition===yes ? [20,10] : [20,30,10]);
      assert.equal(polls, 1);
    }
  } else if (test.name === 'final_check') {
    for (const failure of [0,1]) {
      lines=[]; polls=0; failAt=failure;
      assert.equal(app.molt_main(), none);
      assert.deepEqual(lines, failure ? [20,10] : [20]);
    }
  } else {
    lines=[]; polls=0; failAt=0;
    assert.equal(app.molt_main(), none); assert.deepEqual(lines, [99]);
  }
}
"#,
        &[&config],
        "dispatch edges execute with exact output and callback order",
    );
}
