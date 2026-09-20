use super::support::*;
use crate::wasm::test_execution::{real_execution_tool, run_node_test_script, wasm_test_temp_dir};
use serde_json::json;
use std::{fs, path::PathBuf};

#[test]
fn hash_constructor_execution_preserves_owner_and_failure_atomicity() {
    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "hash constructor ownership",
    )
    .expect("Node is required to prove emitted constructor ownership");
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    for (kind, insert, width) in [
        ("dict_new", "dict_set", 2),
        ("set_new", "set_add", 1),
        ("frozenset_new", "frozenset_add", 1),
    ] {
        for (shape, out) in [
            ("absent", None),
            ("none", Some("none")),
            ("dead", Some("unused")),
            ("live", Some("result")),
        ] {
            let bound = shape == "live";
            let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["value"],
                    None,
                    vec![
                        wasm_test_op(kind, out, vec!["value"; 2 * width]),
                        wasm_test_op("ret", None, vec![if bound { "result" } else { "none" }]),
                    ],
                )],
                profile: None,
            })
            .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
            let path = temp.join(format!("{kind}-{shape}.wasm"));
            fs::write(&path, wasm).unwrap();
            cases.push(json!({"name":format!("{kind}/{shape}"), "constructor":kind,
                "insert":insert, "width":width, "bound":bound, "path":path,
                "memory_pages":memory_pages, "table_entries":table_entries}));
        }
    }
    let config = temp.join("hash-construction.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases":cases, "none":molt_codegen_abi::box_none_bits().to_string()
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), owner = 777n, value = 23n;
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  for (const failure of ['success', 'allocate', 'first', 'second']) {
    const name = test.name + '/' + failure;
    let owners = 0, inserted = 0, pending = false, allocations = 0;
    const imports = {env: {
      memory: new WebAssembly.Memory({initial:test.memory_pages}),
      __indirect_function_table: new WebAssembly.Table({initial:test.table_entries, element:'anyfunc'}),
    }};
    const providers = {
      [test.constructor]: capacity => {
        assert.equal(capacity, 2n, name);
        allocations++;
        if (failure === 'allocate') { pending = true; return none; }
        owners++;
        return owner;
      },
      [test.insert]: (receiver, ...args) => {
        assert.equal(receiver, owner, name + ': lost container identity');
        assert.equal(owners, 1, name + ': container must remain alive');
        assert.equal(pending, false, name + ': called after failure');
        assert.equal(args.length, test.width, name);
        for (const arg of args) assert.equal(arg, value, name);
        inserted++;
        pending = (failure === 'first' && inserted === 1) || (failure === 'second' && inserted === 2);
        return test.insert === 'dict_set' && !pending ? owner : none;
      },
      exception_pending: () => pending ? 1n : 0n,
      dec_ref_obj: bits => {
        if (bits === none) return;
        assert.equal(bits, owner, name + ': unexpected release');
        assert.equal(owners, 1, name + ': duplicate release');
        owners--;
      },
    };
    for (const entry of WebAssembly.Module.imports(module)) {
      imports[entry.module] ??= {};
      if (entry.kind !== 'function') {
        assert.ok(entry.module === 'env' && entry.name in imports.env, name + ': unknown host resource');
        continue;
      }
      imports[entry.module][entry.name] = providers[entry.name] ??
        (() => { throw new Error(name + ': unexpected runtime call ' + entry.name); });
    }
    const app = new WebAssembly.Instance(module, imports).exports;
    const result = app.molt_main(value);
    const retained = test.bound && failure === 'success';
    assert.equal(result, retained ? owner : none, name + ': result');
    assert.equal(allocations, 1, name);
    assert.equal(inserted, failure === 'allocate' ? 0 : failure === 'first' ? 1 : 2, name);
    assert.equal(pending, failure !== 'success', name + ': exception preserved');
    assert.equal(owners, retained ? 1 : 0, name + ': owner balance');
    if (retained) providers.dec_ref_obj(result);
    assert.equal(owners, 0, name + ': caller releases final owner');
  }
}
"#,
        &[&config],
        "WASM hash constructor ownership and failure atomicity",
    );
}

fn compile_literal_body(params: Vec<&str>, ops: Vec<OpIR>) -> (Vec<String>, BTreeMap<String, u32>) {
    let ir = SimpleIR {
        functions: vec![wasm_test_function("molt_main", params, None, ops)],
        profile: None,
    };
    let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .unwrap();
    (
        wasm_operator_debug_for_export(&output.wasm, "molt_main"),
        wasm_function_import_indices(&output.wasm),
    )
}

fn call_count(operators: &[String], function_index: u32) -> usize {
    let call = format!("Call {{ function_index: {function_index} }}");
    operators
        .iter()
        .filter(|operator| operator.as_str() == call.as_str())
        .count()
}

#[test]
fn raw_allocations_publish_before_binding_or_releasing_the_result() {
    for kind in ["alloc", "alloc_class"] {
        for result in [None, Some("none"), Some("result")] {
            let mut allocation = wasm_test_op(
                kind,
                result,
                if kind == "alloc_class" {
                    vec!["class"]
                } else {
                    vec![]
                },
            );
            allocation.value = Some(8);
            let (operators, imports) = compile_literal_body(
                vec!["class"],
                vec![
                    allocation,
                    wasm_test_op(
                        "ret",
                        None,
                        vec![result.filter(|name| *name != "none").unwrap_or("none")],
                    ),
                ],
            );
            let allocate = imports[kind];
            let publish = imports["object_publish_initialized"];
            let position = operators
                .iter()
                .position(|operator| operator == &format!("Call {{ function_index: {allocate} }}"))
                .expect("allocation must be emitted");
            assert_eq!(
                operators[position + 1],
                format!("Call {{ function_index: {publish} }}")
            );
            assert_eq!(call_count(&operators, publish), 1, "{kind} {result:?}");
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(result != Some("result")),
                "{kind} {result:?}: {operators:?}"
            );
        }
    }
}

#[test]
fn owned_runtime_and_constructor_results_release_every_discard_shape() {
    for (kind, argc, temporary_releases) in [
        ("list_new", 0, 0),
        ("tuple_new", 0, 0),
        ("dict_new", 0, 0),
        ("set_new", 0, 0),
        ("frozenset_new", 0, 0),
        ("dataclass_new", 4, 0),
        ("dataclass_new_values", 3, 1),
        ("tuple_index", 2, 0),
        ("get_attr_name", 2, 0),
        ("class_new", 1, 0),
        ("gen_send", 2, 0),
        ("gen_throw", 2, 0),
        ("gen_close", 1, 0),
        ("iter_next", 1, 0),
        ("gpu_thread_id", 0, 0),
    ] {
        for result in [None, Some("none"), Some("unused"), Some("result")] {
            let retained = result == Some("result");
            let (operators, imports) = compile_literal_body(
                vec!["value"],
                vec![
                    wasm_test_op(kind, result, vec!["value"; argc]),
                    wasm_test_op("ret", None, vec![if retained { "result" } else { "none" }]),
                ],
            );
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                temporary_releases + usize::from(!retained),
                "{kind} {result:?}: {operators:?}"
            );
        }
    }
}

#[test]
fn numeric_runtime_results_follow_generated_ownership_for_every_result_shape() {
    use crate::wasm_abi_generated::{
        STATIC_FUNC_TYPES, WASM_NUMERIC_RUNTIME_SELECTORS, WasmRuntimeReturn,
    };

    let node = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "numeric runtime result ownership",
    )
    .expect("Node is required to prove emitted numeric result ownership");
    let (temp, _remove_temp) = wasm_test_temp_dir();
    let mut cases = Vec::new();
    assert!(!WASM_NUMERIC_RUNTIME_SELECTORS.is_empty());
    for spec in WASM_NUMERIC_RUNTIME_SELECTORS {
        let import = spec.selection.import;
        let signature = &STATIC_FUNC_TYPES[import.type_idx() as usize];
        assert_eq!(import.return_contract(), WasmRuntimeReturn::OwnedObject);
        assert!(
            signature
                .params
                .iter()
                .all(|ty| *ty == wasm_encoder::ValType::I64)
        );
        for (shape, out) in [
            ("absent", None),
            ("none", Some("none")),
            ("dead", Some("unused")),
            ("live", Some("result")),
        ] {
            let bound = shape == "live";
            let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["value"],
                    None,
                    vec![
                        wasm_test_op(spec.kind, out, vec!["value"; signature.params.len()]),
                        wasm_test_op("ret", None, vec![if bound { "result" } else { "none" }]),
                    ],
                )],
                profile: None,
            })
            .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let (memory_pages, table_entries) = wasm_import_minimums(&wasm);
            let path = temp.join(format!("{}-{shape}.wasm", spec.kind));
            fs::write(&path, wasm).unwrap();
            cases.push(json!({
                "name": format!("{}-{shape}", spec.kind), "import": import.name(),
                "arity": signature.params.len(), "bound": bound, "path": path,
                "memory_pages": memory_pages, "table_entries": table_entries,
            }));
        }
    }
    let config = temp.join("numeric-results.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "cases": cases, "none": molt_codegen_abi::box_none_bits().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const config = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const none = BigInt(config.none), owner = 777n;
for (const test of config.cases) {
  const module = new WebAssembly.Module(fs.readFileSync(test.path));
  for (const failed of [false, true]) {
    const name = test.name + (failed ? '/failed' : '/success');
    let owners = 0, calls = 0;
    const imports = {env: {
      memory: new WebAssembly.Memory({initial:test.memory_pages}),
      __indirect_function_table: new WebAssembly.Table({initial:test.table_entries, element:'anyfunc'}),
    }};
    const providers = {
      [test.import]: (...args) => {
        calls++;
        assert.equal(args.length, test.arity, name + ': arity');
        for (const arg of args) assert.equal(arg, none, name + ': operand');
        if (failed) return none;
        owners++;
        return owner;
      },
      dec_ref_obj: bits => {
        if (bits === none) return;
        assert.equal(bits, owner, name + ': wrong result released');
        assert.equal(owners, 1, name + ': duplicate release');
        owners--;
      },
    };
    for (const entry of WebAssembly.Module.imports(module)) {
      imports[entry.module] ??= {};
      if (entry.kind !== 'function') {
        assert.ok(entry.module === 'env' && entry.name in imports.env, name + ': unknown host resource');
        continue;
      }
      imports[entry.module][entry.name] = providers[entry.name] ??
        (() => { throw new Error(name + ': unexpected runtime call ' + entry.name); });
    }
    // A tagged nonnumeric operand selects the boxed runtime path. This test
    // proves call/result ownership, not the provider's arithmetic semantics.
    const result = new WebAssembly.Instance(module, imports).exports.molt_main(none);
    const retained = test.bound && !failed;
    assert.equal(calls, 1, name + ': selected runtime import');
    assert.equal(result, retained ? owner : none, name + ': result');
    assert.equal(owners, retained ? 1 : 0, name + ': owner balance');
    if (retained) providers.dec_ref_obj(result);
    assert.equal(owners, 0, name + ': caller releases final owner');
  }
}
"#,
        &[&config],
        "WASM numeric runtime result ownership",
    );
}

#[test]
fn raw_and_borrowed_runtime_results_do_not_acquire_discarded_owners() {
    for (kind, argc, borrowed) in [("call", 1, false), ("guard_tag", 2, true)] {
        for result in [None, Some("none"), Some("unused"), Some("result")] {
            let retained = result == Some("result");
            let mut operation = wasm_test_op(kind, result, vec!["value"; argc]);
            if kind == "call" {
                operation.s_value = Some("molt_int_as_i64".into());
            }
            let (operators, imports) = compile_literal_body(
                vec!["value"],
                vec![
                    operation,
                    wasm_test_op("ret", None, vec![if retained { "result" } else { "none" }]),
                ],
            );
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            let retains = imports
                .get("inc_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(releases, 0, "{kind} {result:?}: {operators:?}");
            assert_eq!(
                retains,
                usize::from(borrowed && retained),
                "{kind} {result:?}: {operators:?}"
            );
        }
    }
}

#[test]
fn manual_field_and_closure_consumers_obey_selected_import_ownership() {
    for (kind, argc, import) in [
        ("closure_load", 1, "closure_load"),
        ("closure_store", 2, "closure_store"),
        ("load", 1, "object_field_get"),
        ("guarded_load", 1, "object_field_get"),
        ("store", 2, "object_field_set"),
        ("guarded_field_get", 3, "guarded_field_get"),
        ("guarded_field_set", 4, "guarded_field_set"),
    ] {
        for result in [None, Some("none"), Some("unused"), Some("result")] {
            let bound = result == Some("result") && !matches!(kind, "store" | "guarded_field_set");
            let mut op = wasm_test_op(kind, result, vec!["value"; argc]);
            op.value = Some(0);
            op.s_value = Some("attribute".into());
            let (operators, imports) = compile_literal_body(
                vec!["value"],
                vec![
                    op,
                    wasm_test_op("ret", None, vec![if bound { "result" } else { "none" }]),
                ],
            );
            let call = format!("Call {{ function_index: {} }}", imports[import]);
            let mut found = false;
            for (index, operator) in operators.iter().enumerate() {
                if operator != &call {
                    continue;
                }
                found = true;
                if bound {
                    assert!(
                        operators[index + 1].starts_with("LocalSet {"),
                        "{kind}: {operators:?}"
                    );
                } else {
                    assert_eq!(
                        operators[index + 1],
                        format!("Call {{ function_index: {} }}", imports["dec_ref_obj"]),
                        "{kind} {result:?}: {operators:?}"
                    );
                }
            }
            assert!(found, "{kind}: runtime path must remain");
        }
    }
}

#[test]
fn representation_aliases_retain_only_observable_results() {
    for kind in [
        "box",
        "unbox",
        "cast",
        "widen",
        "binding_alias",
        "and",
        "or",
    ] {
        for result in [None, Some("none"), Some("unused"), Some("result")] {
            let bound = result == Some("result");
            let (operators, imports) = compile_literal_body(
                vec!["value"],
                vec![
                    wasm_test_op(
                        kind,
                        result,
                        vec!["value"; if matches!(kind, "and" | "or") { 2 } else { 1 }],
                    ),
                    wasm_test_op("ret", None, vec![if bound { "result" } else { "none" }]),
                ],
            );
            let retains = imports
                .get("inc_ref_obj")
                .map_or(0, |&i| call_count(&operators, i));
            assert_eq!(
                retains,
                usize::from(bound),
                "{kind} {result:?}: {operators:?}"
            );
        }
    }
}

#[test]
fn effect_output_metadata_cannot_publish_a_runtime_value() {
    for (kind, argc, import, borrowed) in [
        ("store_index", 3, "store_index", true),
        ("store", 2, "object_field_set", false),
        ("set_attr_name", 3, "set_attr_name", false),
    ] {
        let mut effect = wasm_test_op(kind, Some("value"), vec!["value"; argc]);
        effect.value = Some(0);
        let (operators, imports) = compile_literal_body(
            vec!["value"],
            vec![effect, wasm_test_op("ret", None, vec!["value"])],
        );
        let call = format!("Call {{ function_index: {} }}", imports[import]);
        let expected = if borrowed {
            "Drop".to_string()
        } else {
            format!("Call {{ function_index: {} }}", imports["dec_ref_obj"])
        };
        for index in operators
            .iter()
            .enumerate()
            .filter_map(|(i, op)| (op == &call).then_some(i))
        {
            assert_eq!(operators[index + 1], expected, "{kind}: {operators:?}");
        }
        let retains = imports
            .get("inc_ref_obj")
            .map_or(0, |&i| call_count(&operators, i));
        assert_eq!(retains, 0, "{kind}: metadata must not acquire an owner");
    }
}

#[test]
fn effect_refcount_metadata_never_overwrites_an_existing_local() {
    for (kind, import) in [("inc_ref", "inc_ref_obj"), ("dec_ref", "dec_ref_obj")] {
        for output in [None, Some("none"), Some("value")] {
            let (operators, imports) = compile_literal_body(
                vec!["value"],
                vec![
                    wasm_test_op(kind, output, vec!["value"]),
                    wasm_test_op("ret", None, vec!["value"]),
                ],
            );
            assert_eq!(call_count(&operators, imports[import]), 1, "{kind}");
            assert!(
                !operators
                    .iter()
                    .any(|op| op.starts_with("LocalSet { local_index: 0 }")
                        || op.starts_with("LocalTee { local_index: 0 }")),
                "{kind} {output:?}: effect metadata cannot overwrite the input: {operators:?}"
            );
        }
    }
}

#[test]
fn loop_index_copies_accept_all_result_shapes() {
    for kind in ["loop_index_start", "loop_index_next"] {
        for output in [None, Some("none"), Some("unused"), Some("result")] {
            let bound = output == Some("result");
            let (operators, _) = compile_literal_body(
                vec!["value"],
                vec![
                    wasm_test_op(kind, output, vec!["value"]),
                    wasm_test_op("ret", None, vec![if bound { "result" } else { "none" }]),
                ],
            );
            assert!(!operators.is_empty());
        }
    }
}

#[test]
fn scalar_parse_families_share_object_admission_and_owned_result_sinks() {
    for kind in ["json_parse", "msgpack_parse", "cbor_parse"] {
        for literal in [None, Some("const_str"), Some("const_bytes")] {
            for result in [None, Some("none"), Some("unused"), Some("result")] {
                let bound = result == Some("result");
                let mut ops = Vec::new();
                if let Some(literal) = literal {
                    let mut input = wasm_test_op(literal, Some("input"), vec![]);
                    input.bytes = Some(b"23".to_vec());
                    ops.push(input);
                }
                ops.push(wasm_test_op(kind, result, vec!["input"]));
                if literal.is_some() {
                    ops.push(wasm_test_op("dec_ref", None, vec!["input"]));
                }
                ops.push(wasm_test_op(
                    "ret",
                    None,
                    vec![if bound { "result" } else { "none" }],
                ));
                let (operators, imports) = compile_literal_body(
                    if literal.is_some() {
                        vec![]
                    } else {
                        vec!["input"]
                    },
                    ops,
                );
                assert!(
                    !imports.contains_key("alloc"),
                    "{kind}: parser buffer is not a language object"
                );
                assert!(
                    !imports.contains_key("handle_resolve"),
                    "{kind}: parser buffer is a raw pointer"
                );
                assert!(!imports.contains_key("scratch_alloc"));
                assert!(!imports.contains_key("scratch_free"));
                assert!(!imports.contains_key(&format!("{kind}_scalar")));
                let return_call = format!(
                    "Call {{ function_index: {} }}",
                    imports[&format!("{kind}_scalar_obj")]
                );
                let index = operators.iter().position(|op| op == &return_call).unwrap();
                assert_eq!(
                    call_count(&operators, imports[&format!("{kind}_scalar_obj")]),
                    1
                );
                if bound {
                    assert!(operators[index + 1].starts_with("LocalSet {"));
                } else {
                    assert_eq!(
                        operators[index + 1],
                        format!("Call {{ function_index: {} }}", imports["dec_ref_obj"])
                    );
                }
            }
        }
    }
}

#[test]
fn direct_calls_release_only_owned_value_results() {
    for (kind, target, argc, owns_result, returns_value) in [
        ("call", "molt_classmethod_new", 1, true, true),
        ("call", "molt_function_closure_bits", 1, false, true),
        ("call", "molt_print_newline", 0, false, false),
        ("call", "owned_call_target", 1, true, true),
        ("call_internal", "owned_call_target", 1, true, true),
        ("call", "external_owned_target", 1, true, true),
        ("call_internal", "external_owned_target", 1, true, true),
        ("call_internal", "external_void_target", 0, false, false),
    ] {
        for result in [None, Some("none"), Some("result")] {
            let bound = result == Some("result");
            if target == "molt_print_newline" && bound {
                continue;
            }
            let mut call = wasm_test_op(kind, result, vec!["value"; argc]);
            call.s_value = Some(target.into());
            let mut external_owned = wasm_test_function(
                "external_owned_target",
                vec!["arg"],
                None,
                vec![wasm_test_op("ret", None, vec!["arg"])],
            );
            external_owned.externalize_with_signature().unwrap();
            let mut external_void = wasm_test_function(
                "external_void_target",
                vec![],
                None,
                vec![wasm_test_op("ret_void", None, vec![])],
            );
            external_void.externalize_with_signature().unwrap();
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["value"],
                        None,
                        vec![
                            call,
                            wasm_test_op("const_none", Some("nothing"), vec![]),
                            wasm_test_op(
                                "ret",
                                None,
                                vec![if bound { "result" } else { "nothing" }],
                            ),
                        ],
                    ),
                    wasm_test_function(
                        "owned_call_target",
                        vec!["arg"],
                        None,
                        vec![wasm_test_op("ret", None, vec!["arg"])],
                    ),
                    external_owned,
                    external_void,
                ],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind} {target}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(owns_result && !bound),
                "{kind} {target}, bound={bound}: {operators:?}"
            );
            let retains = imports
                .get("inc_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                retains,
                usize::from(target == "molt_function_closure_bits" && bound),
                "{kind} {target}, bound={bound}: {operators:?}"
            );
            if !owns_result && returns_value && !bound {
                let target_import = imports["function_closure_bits"];
                let position = operators
                    .iter()
                    .position(|operator| {
                        operator == &format!("Call {{ function_index: {target_import} }}")
                    })
                    .unwrap();
                assert_eq!(operators[position + 1], "Drop", "{operators:?}");
            }
        }
    }
}

#[test]
fn runtime_bootstrap_does_not_retain_live_locals() {
    let compile = |bootstrap: bool| {
        let mut ops = Vec::new();
        if bootstrap {
            let mut init = wasm_test_op("call", None, vec![]);
            init.s_value = Some("molt_runtime_init".into());
            ops.push(init);
        }
        ops.push(wasm_test_op("ret", None, vec!["value"]));
        let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
            functions: vec![wasm_test_function("molt_main", vec!["value"], None, ops)],
            profile: None,
        });
        wasmparser::Validator::new()
            .validate_all(&output.wasm)
            .unwrap();
        let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
        let imports = wasm_function_import_indices(&output.wasm);
        ["inc_ref_obj", "dec_ref_obj"].map(|name| {
            imports
                .get(name)
                .map_or(0, |&index| call_count(&operators, index))
        })
    };
    assert_eq!(compile(true), compile(false));
}

#[test]
fn dynamic_calls_release_discarded_owned_results() {
    for (kind, argc, target) in [
        ("call_func", 1, None),
        ("call_bind", 2, None),
        ("call_indirect", 2, None),
        ("call_guarded", 2, Some("owned_call_target")),
        ("call_method", 1, None),
        ("call_method", 1, Some("BoundMethod:str:upper")),
        ("call_method", 1, Some("BoundMethod:str:lower")),
        ("call_method", 1, Some("BoundMethod:str:strip")),
        ("call_method", 2, Some("BoundMethod:list:append")),
        ("call_method", 2, Some("BoundMethod:str:join")),
        ("call_method", 2, Some("BoundMethod:str:startswith")),
        ("call_method", 3, Some("BoundMethod:dict:get")),
        ("call_method_ic", 1, Some("method")),
        ("call_super_method_ic", 2, Some("method")),
        ("invoke_ffi", 1, None),
    ] {
        for result in [None, Some("none"), Some("result")] {
            let bound = result == Some("result");
            let mut call = wasm_test_op(kind, result, vec!["value"; argc]);
            call.s_value = target.map(str::to_string);
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["value"],
                        None,
                        vec![
                            call,
                            wasm_test_op("const_none", Some("nothing"), vec![]),
                            wasm_test_op(
                                "ret",
                                None,
                                vec![if bound { "result" } else { "nothing" }],
                            ),
                        ],
                    ),
                    wasm_test_function(
                        "owned_call_target",
                        vec!["arg"],
                        None,
                        vec![wasm_test_op("ret", None, vec!["arg"])],
                    ),
                ],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind} {target:?}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(!bound)
                    + ["frame_invocation_exit", "callargs_push_pos"]
                        .iter()
                        .map(|name| {
                            imports
                                .get(*name)
                                .map_or(0, |&index| call_count(&operators, index))
                        })
                        .sum::<usize>(),
                "{kind} {target:?}, bound={bound}: {operators:?}"
            );
        }
    }
}

#[test]
fn native_symbol_results_preserve_owned_and_raw_abis() {
    for (abi, argc, owns_result) in [
        ("molt.object_call_v1", 1, true),
        ("molt.object_callargs_v1", 1, true),
        ("molt.forward_f32_v1", 1, true),
        ("molt.pyinit_module_v1", 0, false),
    ] {
        for result in [None, Some("none"), Some("result")] {
            let bound = result == Some("result");
            let mut call = wasm_test_op("invoke_ffi", result, vec!["value"; argc]);
            call.native_callable_export = Some("native.probe".into());
            call.native_callable_binding = Some("direct_symbol".into());
            call.native_callable_symbol = Some("native_probe".into());
            call.native_callable_abi = Some(abi.into());
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["value"],
                    None,
                    vec![
                        call,
                        wasm_test_op("const_none", Some("nothing"), vec![]),
                        wasm_test_op("ret", None, vec![if bound { "result" } else { "nothing" }]),
                    ],
                )],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{abi}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(owns_result && !bound),
                "{abi}, bound={bound}: {operators:?}"
            );
        }
    }
}

#[test]
fn direct_calls_reject_runtime_void_outputs_and_internal_runtime_targets() {
    for (kind, target, argc) in [
        ("call", "molt_print_newline", 0),
        ("call_internal", "molt_classmethod_new", 1),
    ] {
        let outcome = std::panic::catch_unwind(|| {
            let mut call = wasm_test_op(kind, Some("result"), vec!["arg"; argc]);
            call.s_value = Some(target.into());
            wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["arg"],
                    None,
                    vec![call, wasm_test_op("ret", None, vec!["result"])],
                )],
                profile: None,
            });
        });
        assert!(
            outcome.is_err(),
            "{kind} {target} must reject the invalid result ABI"
        );
    }
}

#[test]
fn callable_constructors_release_only_discarded_owned_results() {
    for (kind, argc) in [
        ("func_new", 0),
        ("func_new_closure", 1),
        ("builtin_func", 0),
        ("builtin_func", 1),
        ("code_new", 9),
        ("callargs_new", 0),
        ("classmethod_new", 1),
        ("staticmethod_new", 1),
        ("property_new", 3),
        ("bound_method_new", 2),
        ("asyncgen_new", 1),
    ] {
        for result in [None, Some("none"), Some("created")] {
            let bound = result == Some("created");
            let mut constructor = wasm_test_op(kind, result, vec!["value"; argc]);
            if matches!(kind, "func_new" | "func_new_closure") {
                constructor.s_value = Some("callable_result_target".into());
                constructor.value = Some(0);
            } else if kind == "builtin_func" {
                // Builtin constructors target the runtime callable manifest,
                // not a compiled FunctionIR body. The optional local supplies
                // a name to FuncNewBuiltinNamed, not another callable argument.
                constructor.s_value = Some("molt_abs_builtin".into());
                constructor.value = Some(1);
            }
            let mut functions = vec![wasm_test_function(
                "molt_main",
                vec!["value"],
                None,
                vec![
                    constructor,
                    wasm_test_op("ret", None, vec![if bound { "created" } else { "value" }]),
                ],
            )];
            if matches!(kind, "func_new" | "func_new_closure") {
                functions.push(wasm_test_function(
                    "callable_result_target",
                    if kind == "func_new_closure" {
                        vec![crate::MOLT_CLOSURE_PARAM_NAME]
                    } else {
                        vec![]
                    },
                    None,
                    vec![
                        wasm_test_op("const_none", Some("nothing"), vec![]),
                        wasm_test_op("ret", None, vec!["nothing"]),
                    ],
                ));
            }
            let ir = SimpleIR {
                functions,
                profile: None,
            };
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let symbol = if kind == "builtin_func" {
                assert!(imports.contains_key("abs_builtin"));
                if argc == 0 {
                    "func_new_builtin"
                } else {
                    "func_new_builtin_named"
                }
            } else {
                kind
            };
            assert_eq!(
                call_count(&operators, imports[symbol]),
                1,
                "{kind}: {operators:?}"
            );
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(!bound),
                "{kind}, bound={bound}: {operators:?}"
            );
            if !bound {
                let call = format!("Call {{ function_index: {} }}", imports[symbol]);
                let position = operators
                    .iter()
                    .position(|operator| operator == &call)
                    .unwrap();
                assert_eq!(
                    operators[position + 1],
                    format!("Call {{ function_index: {} }}", imports["dec_ref_obj"]),
                    "{operators:?}"
                );
            }
        }
    }
}

#[test]
fn closure_extraction_retains_only_bound_borrowed_results() {
    for result in [None, Some("none"), Some("closure")] {
        let bound = result == Some("closure");
        let ir = SimpleIR {
            functions: vec![wasm_test_function(
                "molt_main",
                vec!["callee"],
                None,
                vec![
                    wasm_test_op("function_closure_bits", result, vec!["callee"]),
                    if bound {
                        wasm_test_op("ret", None, vec!["closure"])
                    } else {
                        wasm_test_op("ret_void", None, vec![])
                    },
                ],
            )],
            profile: None,
        };
        let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
        wasmparser::Validator::new()
            .validate_all(&output.wasm)
            .expect("bound and discarded borrowed results must produce valid WASM");
        let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
        let imports = wasm_function_import_indices(&output.wasm);
        let extract = imports["function_closure_bits"];
        assert_eq!(call_count(&operators, extract), 1, "{operators:?}");
        for (symbol, expected) in [
            ("inc_ref_obj", usize::from(bound)),
            ("dec_ref_obj", 0),
            ("dec_ref", 0),
        ] {
            let actual = imports
                .get(symbol)
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(actual, expected, "bound={bound}: {symbol}: {operators:?}");
        }
        let extract_call = format!("Call {{ function_index: {extract} }}");
        let position = operators.iter().position(|op| op == &extract_call).unwrap();
        if bound {
            assert!(
                operators[position + 1].starts_with("LocalTee {"),
                "{operators:?}"
            );
            assert_eq!(
                operators[position + 2],
                format!("Call {{ function_index: {} }}", imports["inc_ref_obj"]),
                "{operators:?}"
            );
        } else {
            assert_eq!(operators[position + 1], "Drop", "{operators:?}");
        }
    }
}

pub(super) fn assert_every_exit_releases_anchor(
    operators: &[String],
    dec_ref_index: u32,
    expected_minimum_returns: usize,
) -> usize {
    let release = format!("Call {{ function_index: {dec_ref_index} }}");
    let mut exit_positions: Vec<usize> = operators
        .iter()
        .enumerate()
        .filter_map(|(index, operator)| (operator == "Return").then_some(index))
        .collect();
    assert!(
        exit_positions.len() >= expected_minimum_returns,
        "expected at least {expected_minimum_returns} return paths; operators={operators:?}"
    );
    assert_eq!(operators.last().map(String::as_str), Some("End"));
    // Plain bodies include an implicit fallthrough epilogue even when the IR
    // ends with an explicit return. Dispatch bodies end in an explicit Return.
    if operators
        .get(operators.len().wrapping_sub(2))
        .map(String::as_str)
        != Some("Return")
    {
        exit_positions.push(operators.len() - 1);
    }
    for &return_index in &exit_positions {
        assert_eq!(
            operators.get(return_index.wrapping_sub(1)),
            Some(&release),
            "every explicit or implicit function exit must release its unique anchor immediately before returning; operators={operators:?}"
        );
    }
    exit_positions.len()
}

#[test]
fn jumpful_literals_share_one_anchor_and_mint_each_dynamic_result_owner() {
    let mut first = wasm_test_op("const_str", Some("first"), vec![]);
    first.s_value = Some("shared-payload".to_string());
    let mut branch = wasm_test_op("br_if", None, vec!["cond"]);
    branch.value = Some(7);
    let mut second = wasm_test_op("const_str", Some("second"), vec![]);
    second.s_value = Some("shared-payload".to_string());
    let mut label = wasm_test_op("label", None, vec![]);
    label.value = Some(7);

    let (operators, imports) = compile_literal_body(
        vec!["cond"],
        vec![
            first,
            wasm_test_op("dec_ref", None, vec!["first"]),
            branch,
            second,
            wasm_test_op("dec_ref", None, vec!["second"]),
            label,
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["string_from_bytes"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(
        call_count(&operators, imports["inc_ref_obj"]),
        2,
        "each original literal op must mint its own result owner from the shared anchor; operators={operators:?}"
    );
    assert_every_exit_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}

#[test]
fn discarded_literal_results_release_their_owner_without_writing_none() {
    for out in ["unused", "none"] {
        let mut literal = wasm_test_op("const_str", Some(out), vec![]);
        literal.s_value = Some("discarded-payload".into());
        let (operators, imports) = compile_literal_body(
            vec![],
            vec![literal, wasm_test_op("ret", None, vec!["none"])],
        );
        assert_eq!(call_count(&operators, imports["inc_ref_obj"]), 1);
        let exits = assert_every_exit_releases_anchor(&operators, imports["dec_ref_obj"], 1);
        assert_eq!(
            call_count(&operators, imports["dec_ref_obj"]),
            exits + 1,
            "{out}: discard must release the site owner as well as its anchor"
        );
    }
}

#[test]
fn full_i64_const_uses_fallible_anchor_instead_of_inline_47_truncation() {
    let mut wide = wasm_test_op("const", Some("wide"), vec![]);
    wide.value = Some(i64::MAX);
    let (operators, imports) = compile_literal_body(
        vec![],
        vec![
            wide,
            wasm_test_op("dec_ref", None, vec!["wide"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["int_from_i64"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(call_count(&operators, imports["inc_ref_obj"]), 1);
    assert!(
        operators
            .iter()
            .any(|operator| operator == &format!("I64Const {{ value: {} }}", i64::MAX)),
        "full-width constant must reach int_from_i64 without a 47-bit mask; operators={operators:?}"
    );
    assert_every_exit_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}
