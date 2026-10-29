use super::support::*;
use crate::ir::ExecutionContextPolicy;
use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

#[test]
fn invocation_anchors_preserve_frames_reentry_and_foreign_unwind() {
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    // Keep the original failure/reentry cohort and add a returning identity
    // cohort with three distinct anchors but four dynamic literal uses.
    for (count, identity) in [(0, false), (1, false), (3, false), (3, true)] {
        for eh in [false, true] {
            let mut ops = vec![OpIR {
                s_value: Some("molt_main".into()),
                ..wasm_test_op(molt_ir::OpKind::TraceEnterSlot, None, vec![])
            }];
            if identity {
                for (payload, out) in [
                    ("anchor-alpha", "unused"),
                    ("anchor-β\0payload", "unused"),
                    ("anchor-gamma-雪", "unused"),
                    ("anchor-β\0payload", "returned"),
                ] {
                    ops.push(OpIR {
                        s_value: Some(payload.into()),
                        ..wasm_test_op(molt_ir::OpKind::ConstStr, Some(out), vec![])
                    });
                }
                ops.push(wasm_test_op(molt_ir::OpKind::TraceExit, None, vec![]));
                ops.push(wasm_test_op(molt_ir::OpKind::Ret, None, vec!["returned"]));
            } else {
                for index in 0..count {
                    ops.push(OpIR {
                        s_value: Some(format!("private-anchor-{index}")),
                        ..wasm_test_op(molt_ir::OpKind::ConstStr, Some("unused"), vec![])
                    });
                }
                ops.extend([
                    wasm_test_op(molt_ir::OpKind::TraceExit, None, vec![]),
                    wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
                ]);
            }
            let mut function = wasm_test_function("molt_main", vec![], None, ops);
            function.execution_context = ExecutionContextPolicy::Local;
            let ir = SimpleIR {
                functions: vec![function],
                profile: None,
            };
            let trampolines = crate::wasm::trampoline_analysis::analyze_wasm_trampolines(&ir);
            let wasm = WasmBackend::with_options(WasmCompileOptions {
                native_eh_enabled: eh,
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
                crate::wasm::final_functions::WasmFinalFunctions::from_codegen_fixtures(ir),
                trampolines,
                crate::wasm_plan::WasmStageAudit::from_environment(),
            )
            .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let imports = wasm_function_import_indices(&wasm);
            let operators = wasm_operator_debug_for_export(&wasm, "molt_main");
            for name in ["scratch_alloc", "scratch_free"] {
                if count == 0 {
                    assert!(
                        !imports.contains_key(name),
                        "empty anchor family adds no allocator import"
                    );
                } else {
                    let call = format!("Call {{ function_index: {} }}", imports[name]);
                    assert!(operators.iter().any(|op| op == &call));
                }
            }
            let (pages, entries) = wasm_import_minimums(&wasm);
            let path = temp.join(format!("anchor-{count}-{eh}-{identity}.wasm"));
            fs::write(&path, &wasm).unwrap();
            cases.push(
                json!({"path":path,"count":count,"eh":eh,"identity":identity,
                "wrong_slot":false,"pages":pages,"entries":entries}),
            );
            if identity && eh {
                // Mutate only the third anchor's two use loads, not its
                // constructor store or cleanup. This valid-WASM control would
                // pass a liveness-only INCREF ledger while selecting slot zero.
                let mut wrong = wasm.clone();
                let target = wasm_function_export_indices(&wasm)["molt_main"]
                    - wasm_function_import_names(&wasm).len() as u32;
                let mut body_index = 0;
                let mut changed = 0;
                for payload in wasmparser::Parser::new(0).parse_all(&wasm) {
                    if let wasmparser::Payload::CodeSectionEntry(body) = payload.unwrap() {
                        if body_index == target {
                            let mut reader = body.get_operators_reader().unwrap();
                            while !reader.eof() {
                                if let wasmparser::Operator::I64Load { memarg } =
                                    reader.read().unwrap()
                                    && memarg.memory == 0
                                    && memarg.offset == 16
                                {
                                    assert_eq!(memarg.align, 3);
                                    let offset_byte = usize::try_from(reader.original_position())
                                        .expect("validated WASM byte offset fits usize")
                                        - 1;
                                    assert_eq!(
                                        wasm[offset_byte], 16,
                                        "single-byte offset encoding"
                                    );
                                    wrong[offset_byte] = 0;
                                    changed += 1;
                                }
                            }
                        }
                        body_index += 1;
                    }
                }
                assert_eq!(changed, 2, "bounded third-anchor retain/copy load control");
                wasmparser::Validator::new().validate_all(&wrong).unwrap();
                let wrong_path = temp.join("anchor-wrong-slot.wasm");
                fs::write(&wrong_path, wrong).unwrap();
                cases.push(
                    json!({"path":wrong_path,"count":count,"eh":eh,"identity":true,
                    "wrong_slot":true,"pages":pages,"entries":entries}),
                );
            }
        }
    }
    let config = temp.join("anchor-storage.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases":cases,"none":molt_codegen_abi::box_none_bits().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &PathBuf::from("node"),
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  for (const mode of ['normal', 'scratch', 'partial', 'reentry', 'foreign-constructor', 'foreign-cleanup', 'foreign-exit']) {
    if (test.identity && mode !== 'normal') continue;
    if (!test.count && mode !== 'normal') continue;
    if (mode.startsWith('foreign-') && !test.eh) continue;
    const memory = new WebAssembly.Memory({initial:test.pages + 1});
    const imports = {env:{memory, __indirect_function_table:new WebAssembly.Table({initial:test.entries,element:'anyfunc'})}};
    const contexts = [], regions = new Map(), owners = new Map();
    let nextAddress = test.pages * 65536 + 64, nextOwner = 101n, frameDepth = 7, app;
    const foreign = new WebAssembly.Exception(new WebAssembly.Tag({parameters:[]}), []);
    const wrongSlot = new WebAssembly.Exception(new WebAssembly.Tag({parameters:[]}), []);
    // Expected source payloads and use order are independent of emitted slots,
    // constructor encounter identities, and the mutated binary.
    const payloads = test.identity
      ? ['anchor-alpha', 'anchor-β\0payload', 'anchor-gamma-雪']
      : Array.from({length:test.count}, (_, i) => `private-anchor-${i}`);
    const usePayloads = test.identity ? [payloads[0], payloads[1], payloads[2], payloads[1]] : payloads;
    const current = () => contexts[contexts.length - 1];
    const view = () => new DataView(memory.buffer);
    const invoke = (scenario) => {
      const ctx = {scenario, made:[], destroyed:[], constructed:0, pending:false, ptr:null,
                   byPayload:new Map(), uses:[], events:[], mismatch:null, callerReleasing:false,
                   allocations:0, frees:0, entered:0, exited:0, interrupted:false, reentered:false};
      const callerDepth = frameDepth;
      contexts.push(ctx);
      let thrown = null, result;
      try { result = app.molt_main(); } catch (error) { thrown = error; }
      contexts.pop();
      const foreignExpected = scenario.startsWith('foreign-');
      if (test.wrong_slot) {
        assert.equal(thrown, wrongSlot, 'negative control must fail at exact-use identity');
        assert.deepEqual(ctx.mismatch, {index:2, expected:ctx.byPayload.get(payloads[2]), actual:ctx.byPayload.get(payloads[0])});
        assert.equal(ctx.uses.length, 2, 'wrong slot rejected before retaining it');
      } else {
        assert.equal(thrown, foreignExpected ? foreign : null, scenario + ': original exception identity');
        if (!foreignExpected) assert.equal(result, test.identity ? ctx.byPayload.get(payloads[1]) : none);
      }
      assert.equal(frameDepth, callerDepth, scenario + ': caller frame retained');
      assert.equal(ctx.entered, ctx.exited, scenario + ': one exit per entry attempt');
      if (['scratch','partial','foreign-constructor'].includes(scenario)) assert.equal(ctx.entered, 0);
      else assert.equal(ctx.entered, 1);
      assert.equal(ctx.pending, ['scratch','partial'].includes(scenario));
      assert.equal(ctx.allocations, test.count ? 1 : 0);
      assert.equal(ctx.frees, test.count && scenario !== 'scratch' ? 1 : 0);
      if (ctx.ptr !== null) assert.equal(regions.has(ctx.ptr), false);
      if (test.identity && !test.wrong_slot) {
        const [a,b,c] = payloads.map(payload => ctx.byPayload.get(payload));
        assert.equal(ctx.constructed, 3, 'repeated payload must be constructed only once');
        assert.deepEqual(ctx.uses, [a,b,c,b], 'every use selects its exact source payload owner');
        assert.deepEqual(ctx.events, [
          ['retain',a],['release',a],['retain',b],['release',b],
          ['retain',c],['release',c],['retain',b],
          ['release',c],['release',b],['release',a],
        ], 'one result owner per use and one reverse anchor release per distinct payload');
        assert.deepEqual(ctx.destroyed, [c,a], 'returned owner outlives anchor cleanup');
        assert.equal(result,b); assert.equal(owners.get(result),1, 'exactly one caller owner remains after region free');
        assert.equal(ctx.frees,1); assert.equal(frameDepth,callerDepth);
        ctx.callerReleasing=true; contexts.push(ctx);
        hooks.dec_ref_obj(result);
        contexts.pop();
        assert.deepEqual(ctx.destroyed,[c,a,b], 'caller performs final destruction after return');
        assert.deepEqual(ctx.events.at(-1),['release',b]);
      } else {
        assert.deepEqual(ctx.destroyed, [...ctx.made].reverse(), scenario + ': reverse owner destruction');
      }
      for (const owner of ctx.made) assert.equal(owners.get(owner), 0);
      return ctx;
    };
    const hooks = {
      scratch_alloc(size) {
        const ctx = current(); ctx.allocations++;
        assert.equal(size, BigInt(test.count * 8));
        if (ctx.scenario === 'scratch') { ctx.pending = true; return 0n; }
        const ptr = BigInt(nextAddress); nextAddress += Number(size) + 64;
        assert.equal(regions.has(ptr), false); regions.set(ptr, {size,ctx}); ctx.ptr = ptr;
        new Uint8Array(memory.buffer, Number(ptr), Number(size)).fill(0xa5);
        return ptr;
      },
      scratch_free(ptr, size) {
        const ctx = current(); assert.deepEqual(regions.get(ptr), {size,ctx});
        for (let i=0;i<test.count;i++) assert.equal(view().getBigInt64(Number(ptr)+8*i,true), none);
        const escaping = test.identity && !test.wrong_slot ? ctx.byPayload.get(payloads[1]) : null;
        for (const owner of ctx.made) assert.equal(owners.get(owner), owner === escaping ? 1 : 0,
          'region free retires anchors while preserving the returned caller owner');
        if (test.identity) new Uint8Array(memory.buffer, Number(ptr), Number(size)).fill(0xdd);
        regions.delete(ptr); ctx.frees++;
      },
      string_from_bytes(ptr, len, out) {
        const ctx = current(); assert.equal(ctx.pending, false); assert.ok(regions.has(ctx.ptr));
        // An independent owner ledger supplies expected contents, including all
        // future holes; freshly allocated bytes were deliberately nonzero.
        for (let i=0;i<test.count;i++) assert.equal(view().getBigInt64(Number(ctx.ptr)+8*i,true), ctx.made[i] ?? none);
        const expectedPayload = payloads[ctx.constructed];
        assert.notEqual(expectedPayload, undefined, 'no duplicate payload construction');
        const actualBytes = Buffer.from(new Uint8Array(memory.buffer, Number(ptr), Number(len)));
        assert.deepEqual(actualBytes, Buffer.from(expectedPayload, 'utf8'), 'constructor receives exact source bytes including embedded NUL and UTF-8');
        ctx.constructed++; view().setBigInt64(out, none, true);
        const failAt = Math.min(2, test.count);
        if (ctx.constructed === failAt && ctx.scenario === 'partial') { ctx.pending=true; return 2; }
        if (ctx.constructed === failAt && ctx.scenario === 'foreign-constructor') throw foreign;
        const owner = nextOwner++; ctx.made.push(owner); owners.set(owner,1);
        assert.equal(ctx.byPayload.has(expectedPayload),false); ctx.byPayload.set(expectedPayload,owner);
        view().setBigInt64(out,owner,true); return 0;
      },
      exception_pending: () => current().pending ? 1n : 0n,
      exception_pending_fast: () => current().pending ? 1n : 0n,
      trace_enter_slot(slot) {
        assert.equal(slot,0n); const ctx=current(); assert.equal(ctx.pending,false);
        ctx.entered++; frameDepth++; return none;
      },
      trace_exit() {
        const ctx=current(); assert.equal(ctx.entered-ctx.exited,1);
        assert.equal(ctx.frees,test.count ? 1 : 0,'region retired before frame exit');
        ctx.exited++;frameDepth--;
        if (ctx.scenario==='foreign-exit') throw foreign;
        return none;
      },
      inc_ref_obj(owner) {
        const ctx=current(), index=ctx.uses.length, expected=ctx.byPayload.get(usePayloads[index]);
        assert.notEqual(expected,undefined,'no unexpected extra literal retain');
        if (owner !== expected) { ctx.mismatch={index,expected,actual:owner}; throw wrongSlot; }
        assert.ok(owners.get(owner)>0); ctx.uses.push(owner); ctx.events.push(['retain',owner]);
        owners.set(owner,owners.get(owner)+1);
      },
      dec_ref_obj(owner) {
        if (owner===none) return;
        const ctx=current(); assert.ok(ctx.made.includes(owner)); assert.ok(owners.get(owner)>0);
        ctx.events.push(['release',owner]);
        if (ctx.callerReleasing) {
          assert.equal(regions.has(ctx.ptr),false,'caller release cannot use retired anchor memory');
          assert.equal(owners.get(owner),1); owners.set(owner,0); ctx.destroyed.push(owner); return;
        }
        const n=owners.get(owner)-1; owners.set(owner,n);
        if (n) return;
        const slot=ctx.made.indexOf(owner);
        assert.equal(view().getBigInt64(Number(ctx.ptr)+8*slot,true),none,'clear before callback');
        ctx.destroyed.push(owner);
        if (ctx.scenario==='reentry' && !ctx.reentered) {
          ctx.reentered=true; const outer=ctx.ptr; invoke('normal');
          assert.ok(regions.has(outer),'nested invocation cannot retire outer storage');
        }
        if (ctx.scenario==='foreign-cleanup' && !ctx.interrupted) {ctx.interrupted=true;throw foreign;}
      },
    };
    for (const entry of WebAssembly.Module.imports(module)) {
      imports[entry.module] ??= {};
      if (entry.kind==='tag') {imports[entry.module][entry.name]=new WebAssembly.Tag({parameters:['i64']});continue;}
      if (entry.kind!=='function') {assert.ok(entry.name in imports[entry.module]);continue;}
      imports[entry.module][entry.name]=hooks[entry.name] ?? (()=>{throw new Error('unexpected call '+entry.name);});
    }
    app=new WebAssembly.Instance(module,imports).exports;
    invoke(mode);
    // Repeated calls use the same instance after failure, including EH cleanup.
    invoke('normal');
    assert.equal(regions.size,0);assert.equal(frameDepth,7);
    assert.ok([...owners.values()].every(n=>n===0));
  }
}
"#,
        &[&config],
        "WASM invocation anchor storage ownership and failure boundaries",
    );
}
