use super::support::*;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn direct_wasm_bindings_execute_snapshot_rebind_and_discard_forms() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "local binding transport execution",
    )
    .expect("Node is required to prove emitted local binding values");
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    let mut memory_pages = 0;
    let mut table_entries = 0;
    for labelled in [false, true] {
        for (name, destination, result, returned, rebind, returns_none) in [
            (
                "snapshot",
                Some("slot"),
                Some("snapshot"),
                "snapshot",
                true,
                false,
            ),
            (
                "self_rebind",
                Some("source"),
                Some("snapshot"),
                "snapshot",
                true,
                false,
            ),
            ("binding_out", None, Some("slot"), "slot", false, false),
            ("binding_var", Some("slot"), None, "slot", false, false),
            (
                "same_name",
                Some("slot"),
                Some("slot"),
                "slot",
                false,
                false,
            ),
            ("discard", Some("slot"), Some("none"), "slot", false, false),
            (
                "discard_keeps_none",
                Some("slot"),
                Some("none"),
                "none",
                false,
                true,
            ),
        ] {
            let mut ops = Vec::new();
            if labelled {
                for kind in ["jump", "label"] {
                    let mut marker = wasm_test_op(kind, None, vec![]);
                    marker.value = Some(7);
                    ops.push(marker);
                }
            }
            let mut store = wasm_test_op("store_var", result, vec!["source"]);
            store.var = destination.map(str::to_string);
            ops.push(store);
            if rebind {
                let mut store = wasm_test_op("store_var", None, vec!["replacement"]);
                store.var = destination.map(str::to_string);
                ops.push(store);
            }
            ops.push(wasm_test_op("ret", None, vec![returned]));
            let ir = SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["source", "replacement"],
                    None,
                    ops,
                )],
                profile: None,
            };
            crate::validate_simple_ir(&ir).expect("binding fixture is admitted SimpleIR");
            // Prove the raw final-IR consumer, not a preparatory SSA rewrite
            // that can normalize the missing-result bug away first.
            let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir).wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
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
            let path = temp.join(format!("binding_{name}_{labelled}.wasm"));
            fs::write(&path, wasm).unwrap();
            cases.push(json!({"name": format!("{name}/{labelled}"), "path": path, "returns_none": returns_none}));
        }
    }
    let config = temp.join("local_bindings.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases": cases, "memory_pages": memory_pages, "table_entries": table_entries,
            "first": (1.25_f64.to_bits() as i64).to_string(),
            "second": (2.5_f64.to_bits() as i64).to_string(),
            "none": molt_codegen_abi::box_none_bits().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs');
const assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const first = BigInt(config.first), second = BigInt(config.second), none = BigInt(config.none);
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  const imports = {env: {
    memory: new WebAssembly.Memory({initial: config.memory_pages}),
    __indirect_function_table: new WebAssembly.Table({initial: config.table_entries, element: 'anyfunc'}),
  }};
  // No heap values occur in these transport fixtures. Observe only their
  // scalar retain/release boundaries; all other runtime calls fail loudly.
  const scalarReference = value => assert.ok(value === first || value === second || value === none,
    test.name + ': unexpected reference payload ' + value);
  for (const entry of WebAssembly.Module.imports(module)) {
    imports[entry.module] ??= {};
    if (entry.module === 'env' && entry.kind !== 'function') {
      assert.ok(entry.name in imports.env, test.name + ': unexpected host surface ' + entry.name);
      continue;
    }
    assert.equal(entry.kind, 'function');
    imports[entry.module][entry.name] = ['inc_ref_obj', 'dec_ref_obj'].includes(entry.name)
      ? scalarReference : () => { throw new Error(test.name + ': unexpected runtime call ' + entry.name); };
  }
  const app = new WebAssembly.Instance(module, imports).exports;
  assert.equal(app.molt_main(first, second), test.returns_none ? none : first, test.name);
  assert.equal(app.molt_main(second, first), test.returns_none ? none : second, test.name + ': swapped');
}
"#,
        &[&config],
        "WASM local binding destination/result contract",
    );
}
