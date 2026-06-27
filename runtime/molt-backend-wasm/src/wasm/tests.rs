use super::*;
use crate::wasm_plan::is_production_lir_wasm_fast_path_name;

#[test]
fn production_lir_wasm_fast_path_is_reserved_for_global_builtin_lane() {
    assert!(is_production_lir_wasm_fast_path_name(
        "molt_test____molt_globals_builtin__"
    ));
    assert!(!is_production_lir_wasm_fast_path_name(
        "molt_test_regular_helper"
    ));
    assert!(!is_production_lir_wasm_fast_path_name(
        "molt_test_user_callable"
    ));
}

fn wasm_test_function(
    name: &str,
    params: Vec<&str>,
    param_types: Option<Vec<&str>>,
    ops: Vec<OpIR>,
) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.into_iter().map(str::to_string).collect(),
        ops,
        param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
        source_file: None,
        is_extern: false,
    }
}

fn wasm_test_op(kind: &str, out: Option<&str>, args: Vec<&str>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        out: out.map(str::to_string),
        args: Some(args.into_iter().map(str::to_string).collect()),
        ..OpIR::default()
    }
}

#[test]
fn scalar_fast_path_ignores_transport_hints() {
    let mut add = wasm_test_op("add", Some("sum"), vec!["lhs", "rhs"]);
    add.fast_int = Some(true);
    add.type_hint = Some("int".to_string());
    let func = wasm_test_function("hinted", vec!["lhs", "rhs"], None, vec![add.clone()]);
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(!wasm_scalar_integer_fast_path_for_op(&plan, &add));
}

#[test]
fn scalar_fast_path_uses_typed_operands_without_transport_hints() {
    let add = wasm_test_op("add", Some("sum"), vec!["lhs", "rhs"]);
    let mul = wasm_test_op("mul", Some("product"), vec!["lhs", "rhs"]);
    let div = wasm_test_op("div", Some("quot"), vec!["lhs", "rhs"]);
    let func = wasm_test_function(
        "typed",
        vec!["lhs", "rhs"],
        Some(vec!["int", "int"]),
        vec![add.clone(), mul.clone(), div.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &add));
    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &mul));
    assert!(wasm_scalar_integer_fast_path_for_op(&plan, &div));
    assert!(wasm_scalar_truthiness_fast_path_for_name(&plan, "lhs"));
}

#[test]
fn scalar_fast_path_keeps_list_repeat_on_runtime_mul() {
    let list_new = wasm_test_op("list_new", Some("items"), vec!["item"]);
    let repeat = wasm_test_op("mul", Some("repeated"), vec!["items", "count"]);
    let func = wasm_test_function(
        "list_repeat",
        vec!["item", "count"],
        Some(vec!["bool", "int"]),
        vec![list_new, repeat.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(!wasm_scalar_integer_fast_path_for_op(&plan, &repeat));
}

#[test]
fn container_import_selection_ignores_transport_hints() {
    let mut index = wasm_test_op("index", Some("item"), vec!["xs", "i"]);
    index.container_type = Some("list".to_string());
    index.type_hint = Some("list".to_string());
    let func = wasm_test_function(
        "hinted_container",
        vec!["xs", "i"],
        None,
        vec![index.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(
        wasm_specialized_container_import(&plan, 0, "index", &index),
        None
    );
}

#[test]
fn container_import_selection_uses_typed_container_facts() {
    let index = wasm_test_op("index", Some("item"), vec!["xs", "i"]);
    let set = wasm_test_op("store_index", None, vec!["xs", "i", "v"]);
    let len = wasm_test_op("len", Some("n"), vec!["xs"]);
    let func = wasm_test_function(
        "typed_container",
        vec!["xs", "i", "v"],
        Some(vec!["list[int]", "int", "int"]),
        vec![index.clone(), set.clone(), len.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(
        wasm_specialized_container_import(&plan, 0, "index", &index),
        None,
        "semantic list[int] is not a physical flat-list storage proof"
    );
    assert_eq!(
        wasm_specialized_container_import(&plan, 1, "store_index", &set),
        None,
        "semantic list[int] is not a physical flat-list storage proof"
    );
    assert_eq!(
        wasm_specialized_container_import(&plan, 2, "len", &len),
        Some("len_list")
    );
}

#[test]
fn container_import_selection_uses_flat_list_storage_proof() {
    let make = wasm_test_op("list_int_new", Some("xs"), vec!["n"]);
    let index = wasm_test_op("index", Some("item"), vec!["xs", "i"]);
    let set = wasm_test_op("store_index", None, vec!["xs", "i", "v"]);
    let func = wasm_test_function(
        "flat_list_storage",
        vec!["n", "i", "v"],
        Some(vec!["int", "int", "int"]),
        vec![make, index.clone(), set.clone()],
    );
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert_eq!(
        wasm_specialized_container_import(&plan, 1, "index", &index),
        Some("list_int_getitem")
    );
    assert_eq!(
        wasm_specialized_container_import(&plan, 2, "store_index", &set),
        Some("list_int_setitem")
    );
}

/// Extract `(param_count, result_count)` for every func type in a module's
/// type section, in section order.
fn wasm_function_import_names(wasm: &[u8]) -> Vec<String> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            for import in reader.into_imports().flatten() {
                if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                    imports.push(import.name.to_string());
                }
            }
        }
    }
    imports
}

fn wasm_function_import_type_indices(wasm: &[u8]) -> BTreeMap<String, u32> {
    let mut imports = BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            for import in reader.into_imports().flatten() {
                let type_idx = match import.ty {
                    TypeRef::Func(idx) | TypeRef::FuncExact(idx) => idx,
                    _ => continue,
                };
                imports.insert(import.name.to_string(), type_idx);
            }
        }
    }
    imports
}

fn wasm_type_section_signatures(wasm: &[u8]) -> Vec<(usize, usize)> {
    use wasmparser::CompositeInnerType;
    let mut sigs = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::TypeSection(reader)) = payload {
            for rec_group in reader.into_iter() {
                let rec_group = rec_group.expect("valid rec group");
                for sub_type in rec_group.into_types() {
                    if let CompositeInnerType::Func(f) = &sub_type.composite_type.inner {
                        sigs.push((f.params().len(), f.results().len()));
                    }
                }
            }
        }
    }
    sigs
}

#[test]
fn import_transaction_callable_wrapper_matches_runtime_import_abi() {
    let mut import_transaction = wasm_test_op("builtin_func", Some("fn"), vec![]);
    import_transaction.s_value = Some("molt_importlib_import_transaction".to_string());
    import_transaction.value = Some(5);
    let func = wasm_test_function(
        "import_transaction_callable",
        vec![],
        None,
        vec![import_transaction, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("import transaction wrapper must be structurally valid WASM");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("importlib_import_transaction")
        .expect("import transaction runtime import must be registered");
    assert_eq!(
        sigs[import_type as usize],
        (5, 1),
        "importlib_import_transaction import ABI must consume the five values emitted by its callable wrapper"
    );
}

#[test]
fn shared_drop_fact_marker_set_is_explicit_for_wasm() {
    assert!(is_shared_drop_fact_marker("drop_inserted"));
    assert!(is_shared_drop_fact_marker(
        "exception_region_drops_inserted"
    ));
    assert!(!is_shared_drop_fact_marker("inc_ref"));
    assert!(!is_shared_drop_fact_marker("dec_ref"));
    assert!(!is_shared_drop_fact_marker("release"));
}

#[test]
fn generic_wasm_exception_pop_then_drop_keeps_dec_ref_import_across_eh_modes() {
    let mut owned = wasm_test_op("const_str", Some("v0"), vec![]);
    owned.s_value = Some("owned".to_string());
    let func = wasm_test_function(
        "exception_drop",
        vec![],
        None,
        vec![
            wasm_test_op("exception_region_drops_inserted", None, vec![]),
            owned,
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("dec_ref", None, vec!["v0"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    for (native_eh_enabled, expect_exception_pop) in [(true, false), (false, true)] {
        let options = WasmCompileOptions {
            native_eh_enabled,
            reloc_enabled: false,
            ..WasmCompileOptions::default()
        };
        let wasm = WasmBackend::with_options(options).compile(ir.clone());
        let imports = wasm_function_import_names(&wasm);
        assert_eq!(
            imports.iter().any(|name| name == "exception_pop"),
            expect_exception_pop,
            "generic WASM exception_pop import mismatch for native_eh_enabled={native_eh_enabled}; imports={imports:?}"
        );
        assert!(
            imports.iter().any(|name| name == "dec_ref_obj"),
            "generic WASM shared drops must keep dec_ref_obj import for native_eh_enabled={native_eh_enabled}; imports={imports:?}"
        );
    }
}

#[test]
fn wasm_compiles_exception_stack_depth_bookkeeping_family() {
    // Every function with try/with handlers — including the always-present
    // module-globals scaffold — emits the runtime exception-handler-stack depth
    // bookkeeping family (enter/depth/set_depth/exit). Before these handlers
    // existed, WASM codegen panicked in emit_control_op on the very first op
    // (`exception_stack_enter`) of `m____molt_globals_builtin__`, so the backend
    // could not compile ANY program. This compiles the full family and asserts
    // each op lowers to its `molt_exception_stack_*` runtime import with the ABI
    // signature shared with the native backend (no-arg enter/depth -> i64;
    // one-arg exit/set_depth -> i64).
    let func = wasm_test_function(
        "exc_stack_family",
        vec![],
        None,
        vec![
            wasm_test_op("exception_stack_enter", Some("prev"), vec![]),
            wasm_test_op("exception_stack_depth", Some("depth"), vec![]),
            wasm_test_op("exception_stack_set_depth", Some("none"), vec!["depth"]),
            wasm_test_op("exception_stack_exit", Some("none"), vec!["prev"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    // Structural validation catches both the historical codegen panic and any
    // operand-stack imbalance (e.g. a missing Drop on a void-returning op).
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("exception-stack bookkeeping family must compile to structurally valid WASM");

    let import_types = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    for (name, expected_sig) in [
        ("exception_stack_enter", (0usize, 1usize)),
        ("exception_stack_depth", (0, 1)),
        ("exception_stack_exit", (1, 1)),
        ("exception_stack_set_depth", (1, 1)),
    ] {
        let type_idx = *import_types.get(name).unwrap_or_else(|| {
            panic!("{name} runtime import must be registered; imports={import_types:?}")
        });
        assert_eq!(
            sigs[type_idx as usize], expected_sig,
            "{name} import ABI signature mismatch (params, results)"
        );
    }
}
