use super::support::*;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn bind(source: &str, destination: &str) -> OpIR {
    let mut store = wasm_test_op("store_var", None, vec![source]);
    store.var = Some(destination.to_string());
    store
}

fn labelled(kind: &str, label: i64) -> OpIR {
    let mut op = wasm_test_op(kind, None, vec![]);
    op.value = Some(label);
    op
}

fn raw_call(argument: &str, result: &str) -> OpIR {
    let mut call = wasm_test_op("call", Some(result), vec![argument]);
    call.s_value = Some("molt_int_as_i64".into());
    call
}

fn compile_main(params: Vec<&str>, ops: Vec<OpIR>) -> Vec<u8> {
    let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
        functions: vec![wasm_test_function("molt_main", params, None, ops)],
        profile: None,
    })
    .wasm;
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    wasm
}

fn import_call_count(wasm: &[u8], import: &str) -> usize {
    let operators = wasm_operator_debug_for_export(wasm, "molt_main");
    wasm_function_import_indices(wasm)
        .get(import)
        .map_or(0, |&index| {
            let call = format!("Call {{ function_index: {index} }}");
            operators
                .iter()
                .filter(|operator| **operator == call)
                .count()
        })
}

#[test]
fn a_value_released_on_the_calling_path_is_not_retained_across_the_call() {
    // `held` is released on the then-path before the call. Its next read in
    // linear order belongs to the else-path, which the call cannot reach, and
    // the call's result may reuse `held`'s local.
    let wasm = compile_main(
        vec!["cond", "value"],
        vec![
            bind("value", "held"),
            wasm_test_op("if", None, vec!["cond"]),
            wasm_test_op("dec_ref", None, vec!["held"]),
            raw_call("value", "raw"),
            wasm_test_op("ret", None, vec!["raw"]),
            wasm_test_op("else", None, vec![]),
            wasm_test_op("ret", None, vec!["held"]),
            wasm_test_op("end_if", None, vec![]),
        ],
    );
    assert_eq!(import_call_count(&wasm, "inc_ref_obj"), 0);
    assert_eq!(import_call_count(&wasm, "dec_ref_obj"), 1);
}

#[test]
fn a_value_live_across_a_call_is_still_retained() {
    let wasm = compile_main(
        vec!["value"],
        vec![
            bind("value", "kept"),
            raw_call("value", "raw"),
            wasm_test_op("ret", None, vec!["kept"]),
        ],
    );
    assert_eq!(import_call_count(&wasm, "inc_ref_obj"), 1);
    assert_eq!(import_call_count(&wasm, "dec_ref_obj"), 1);
}

/// `seeded` is read only after a jump that skips its definition, so it must
/// observe the dispatch entry seed. `scratch` is written on that path.
fn skipped_definition_body() -> Vec<OpIR> {
    let mut seeded = wasm_test_op("const_float", Some("seeded"), vec![]);
    seeded.f_value = Some(9.5);
    vec![
        labelled("jump", 1),
        labelled("label", 3),
        wasm_test_op("ret", None, vec!["seeded"]),
        labelled("label", 1),
        bind("first", "scratch"),
        bind("scratch", "scratch_copy"),
        labelled("jump", 3),
        seeded,
        wasm_test_op("ret", None, vec!["scratch_copy"]),
    ]
}

/// `held` is observable through both exception transfers; `early` is written
/// between them, while `late` is written after the last one.
fn exception_hole_body() -> Vec<OpIR> {
    vec![
        bind("first", "held"),
        labelled("check_exception", 9),
        bind("second", "early"),
        bind("early", "early_copy"),
        labelled("check_exception", 9),
        bind("second", "late"),
        wasm_test_op("ret", None, vec!["late"]),
        labelled("label", 9),
        wasm_test_op("ret", None, vec!["held"]),
    ]
}

/// `kept` is live around the backward edge; `step` is rewritten every
/// iteration before it is read.
fn jumpful_loop_body() -> Vec<OpIR> {
    let mut again = wasm_test_op("br_if", None, vec!["second"]);
    again.value = Some(5);
    vec![
        bind("first", "kept"),
        labelled("label", 5),
        bind("second", "step"),
        bind("step", "step_copy"),
        again,
        wasm_test_op("ret", None, vec!["kept"]),
    ]
}

fn structured_loop_body() -> Vec<OpIR> {
    vec![
        bind("first", "kept"),
        wasm_test_op("loop_start", None, vec![]),
        bind("second", "step"),
        bind("step", "step_copy"),
        wasm_test_op("loop_break_if_false", None, vec!["second"]),
        wasm_test_op("loop_continue", None, vec![]),
        wasm_test_op("loop_end", None, vec![]),
        wasm_test_op("ret", None, vec!["kept"]),
    ]
}

#[test]
fn shared_locals_execute_entry_seeds_exception_transfers_and_loops() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "shared local storage execution",
    )
    .expect("Node is required to prove emitted shared-local values");
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let first = (1.25_f64.to_bits() as i64).to_string();
    let second = (2.5_f64.to_bits() as i64).to_string();
    let seeded = (9.5_f64.to_bits() as i64).to_string();
    let mut cases = Vec::new();
    for (name, body, fail_at, truthy_calls, expected) in [
        ("entry_seed", skipped_definition_body(), 0, 0, &seeded),
        ("no_exception", exception_hole_body(), 0, 0, &second),
        ("first_transfer", exception_hole_body(), 1, 0, &first),
        ("second_transfer", exception_hole_body(), 2, 0, &first),
        ("jumpful_loop", jumpful_loop_body(), 0, 2, &first),
        ("structured_loop", structured_loop_body(), 0, 2, &first),
    ] {
        let wasm = compile_main(vec!["first", "second"], body);
        let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
        let path = temp.join(format!("{name}.wasm"));
        fs::write(&path, wasm).unwrap();
        cases.push(json!({
            "name": name, "path": path, "fail_at": fail_at, "truthy_calls": truthy_calls,
            "expected": expected, "memory_pages": memory_pages, "table_entries": table_entries,
        }));
    }
    // Undefined entry values use the same boxed None seed regardless of
    // frontend/TIR spelling. The prefix-filtered policy left some as raw zero.
    for name in ["v123", "_v0", "slot", "_bb1_arg0"] {
        let wasm = compile_main(
            vec!["first", "second"],
            vec![labelled("label", 1), wasm_test_op("ret", None, vec![name])],
        );
        let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
        let path = temp.join(format!("undefined_{name}.wasm"));
        fs::write(&path, wasm).unwrap();
        cases.push(json!({
            "name": name, "path": path, "fail_at": 0, "truthy_calls": 0,
            "expected": molt_codegen_abi::box_none_bits().to_string(),
            "memory_pages": memory_pages, "table_entries": table_entries,
        }));
    }
    let config = temp.join("local_storage.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases": cases, "first": first, "second": second, "seeded": seeded,
            "none": molt_codegen_abi::box_none_bits().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const first = BigInt(config.first), second = BigInt(config.second);
const scalars = new Set([first, second, BigInt(config.seeded), BigInt(config.none)]);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  let checks = 0, truths = 0;
  const imports = {env: {
    memory: new WebAssembly.Memory({initial: test.memory_pages}),
    __indirect_function_table: new WebAssembly.Table({initial: test.table_entries, element: 'anyfunc'}),
  }};
  // Values are scalars, so any retain or release must still carry one of them.
  const scalar = bits => assert.ok(scalars.has(bits), test.name + ': unexpected payload ' + bits);
  const providers = {
    exception_pending: () => (++checks === test.fail_at ? 1n : 0n),
    is_truthy: () => (truths++ < test.truthy_calls ? 1n : 0n),
    inc_ref_obj: scalar,
    dec_ref_obj: scalar,
  };
  for (const entry of WebAssembly.Module.imports(module)) {
    imports[entry.module] ??= {};
    if (entry.kind !== 'function') {
      assert.ok(entry.module === 'env' && entry.name in imports.env, test.name + ': unexpected host surface ' + entry.name);
      continue;
    }
    imports[entry.module][entry.name] = providers[entry.name] ??
      (() => { throw new Error(test.name + ': unexpected runtime call ' + entry.name); });
  }
  const app = new WebAssembly.Instance(module, imports).exports;
  assert.equal(app.molt_main(first, second), BigInt(test.expected), test.name);
}
"#,
        &[&config],
        "WASM shared local storage values",
    );
}
