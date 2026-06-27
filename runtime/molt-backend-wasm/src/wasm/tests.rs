use super::local_layout::collect_read_vars;
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

// ---------------------------------------------------------------
// br_table state dispatch
// ---------------------------------------------------------------

#[test]
fn br_table_viable_for_dense_entries() {
    // 6 entries mapping states 0..=5 (dense, above threshold)
    let entries: Vec<(i64, i64)> = (0..6).map(|i| (i as i64, i as i64)).collect();
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_some(), "dense 6-entry range should be viable");
    let (min_state, table_size) = result.unwrap();
    assert_eq!(min_state, 0);
    assert_eq!(table_size, 6);
}

#[test]
fn br_table_viable_with_offset_range() {
    // 5 entries starting at state 10: 10,11,12,13,14
    let entries: Vec<(i64, i64)> = (10..15).map(|i| (i as i64, (i - 10) as i64)).collect();
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_some(), "dense 5-entry range should be viable");
    let (min_state, table_size) = result.unwrap();
    assert_eq!(min_state, 10);
    assert_eq!(table_size, 5);
}

#[test]
fn br_table_rejected_for_few_entries() {
    // Only 4 entries -- below BR_TABLE_MIN_ENTRIES (5)
    let entries: Vec<(i64, i64)> = (0..4).map(|i| (i as i64, i as i64)).collect();
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_none(), "4 entries should be below the threshold");
}

#[test]
fn br_table_rejected_for_sparse_entries() {
    // 5 entries spanning 0..=100: table_size = 101, sparsity = 101/5 = 20.2 (> 8)
    let entries: Vec<(i64, i64)> = vec![(0, 0), (25, 1), (50, 2), (75, 3), (100, 4)];
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_none(), "sparsity 20 exceeds max allowed 8");
}

#[test]
fn br_table_boundary_at_exactly_threshold() {
    // Exactly 5 entries -- the minimum required
    let entries: Vec<(i64, i64)> = (0..5).map(|i| (i as i64, i as i64)).collect();
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_some(), "exactly 5 entries should pass");
    let (min_state, table_size) = result.unwrap();
    assert_eq!(min_state, 0);
    assert_eq!(table_size, 5);
}

#[test]
fn br_table_sparsity_at_max_boundary() {
    // 5 entries, table_size = 5 * 8 = 40 (exactly at sparsity limit)
    // entries: 0, 10, 20, 30, 39  ->  table_size = 40, sparsity = 40/5 = 8
    let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (39, 4)];
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_some(), "sparsity exactly 8 should be accepted");
    let (min_state, table_size) = result.unwrap();
    assert_eq!(min_state, 0);
    assert_eq!(table_size, 40);
}

#[test]
fn br_table_sparsity_just_over_max() {
    // 5 entries, table_size = 41: sparsity = 41/5 = 8.2 (> 8)
    let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (40, 4)];
    let result = br_table_state_remap_params(&entries);
    assert!(result.is_none(), "sparsity 8.2 should be rejected");
}

// ---------------------------------------------------------------
// Dead local elimination -- read-variable scanning
// ---------------------------------------------------------------

/// Build a minimal OpIR with only the fields relevant to read-var scanning.
fn make_op(kind: &str, args: Option<Vec<&str>>, var: Option<&str>, out: Option<&str>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: args.map(|a| a.into_iter().map(String::from).collect()),
        var: var.map(String::from),
        out: out.map(String::from),
        ..Default::default()
    }
}

#[test]
fn read_vars_includes_args_and_var() {
    let ops = vec![
        make_op("add", Some(vec!["a", "b"]), None, Some("c")),
        make_op("load", None, Some("d"), Some("e")),
    ];
    let read_vars = collect_read_vars(&ops);
    assert!(read_vars.contains("a"), "arg 'a' should be in read set");
    assert!(read_vars.contains("b"), "arg 'b' should be in read set");
    assert!(read_vars.contains("d"), "var 'd' should be in read set");
    // 'c' and 'e' are outputs only -- they should NOT be in read_vars
    assert!(
        !read_vars.contains("c"),
        "output-only 'c' should NOT be in read set"
    );
    assert!(
        !read_vars.contains("e"),
        "output-only 'e' should NOT be in read set"
    );
}

#[test]
fn read_vars_output_becomes_live_when_later_read() {
    let ops = vec![
        make_op("const", None, None, Some("x")),
        make_op("add", Some(vec!["x", "y"]), None, Some("z")),
    ];
    let read_vars = collect_read_vars(&ops);
    // 'x' is an output of const but also an arg of add -- should be live
    assert!(
        read_vars.contains("x"),
        "'x' should be live since it's read by add"
    );
    assert!(read_vars.contains("y"), "'y' should be live");
    // 'z' is output-only
    assert!(
        !read_vars.contains("z"),
        "'z' is output-only, should be dead"
    );
}

#[test]
fn dead_local_all_outputs_dead() {
    // No op reads any variable -- all outputs are dead
    let ops = vec![
        make_op("const", None, None, Some("a")),
        make_op("const", None, None, Some("b")),
        make_op("const", None, None, Some("c")),
    ];
    let read_vars = collect_read_vars(&ops);
    assert!(read_vars.is_empty(), "no variable is ever read");
}

#[test]
fn non_linear_control_flow_detection_handles_jumpful_functions() {
    let ops = vec![
        make_op("const", None, None, Some("v0")),
        make_op("check_exception", None, None, None),
        make_op("jump", None, None, None),
        make_op("label", None, None, None),
    ];
    assert!(has_non_linear_control_flow(&ops));
}

#[test]
fn non_linear_control_flow_detection_ignores_straight_line_ops() {
    let ops = vec![
        make_op("const", None, None, Some("v0")),
        make_op("add", Some(vec!["v0", "v1"]), None, Some("v2")),
        make_op("tuple_new", Some(vec!["v2"]), None, Some("v3")),
    ];
    assert!(!has_non_linear_control_flow(&ops));
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
