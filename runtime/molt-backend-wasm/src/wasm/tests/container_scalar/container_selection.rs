use super::*;

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
    let plan = wasm_representation_plan(&func);

    assert_eq!(
        selected_container_runtime_import(&plan, 0, "index", &index),
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
    let plan = wasm_representation_plan(&func);

    assert_eq!(
        selected_container_runtime_import(&plan, 0, "index", &index),
        None,
        "semantic list[int] is not a physical flat-list storage proof"
    );
    assert_eq!(
        selected_container_runtime_import(&plan, 1, "store_index", &set),
        None,
        "semantic list[int] is not a physical flat-list storage proof"
    );
    assert_eq!(
        selected_container_runtime_import(&plan, 2, "len", &len),
        Some(WasmRuntimeImport::LenList)
    );
}

#[test]
fn container_import_selection_uses_manifest_typed_query_matrix() {
    let contains = wasm_test_op("contains", Some("hit"), vec!["xs", "needle"]);
    let len = wasm_test_op("len", Some("n"), vec!["xs"]);
    let cases = [
        (
            "typed_dict_queries",
            "dict",
            Some(WasmRuntimeImport::DictContains),
            Some(WasmRuntimeImport::LenDict),
        ),
        (
            "typed_list_queries",
            "list",
            Some(WasmRuntimeImport::ListContains),
            Some(WasmRuntimeImport::LenList),
        ),
        (
            "typed_set_queries",
            "set",
            Some(WasmRuntimeImport::SetContains),
            Some(WasmRuntimeImport::LenSet),
        ),
        (
            "typed_str_queries",
            "str",
            Some(WasmRuntimeImport::StrContains),
            Some(WasmRuntimeImport::LenStr),
        ),
        (
            "typed_tuple_queries",
            "tuple",
            None,
            Some(WasmRuntimeImport::LenTuple),
        ),
    ];

    for (name, container_type, contains_import, len_import) in cases {
        let func = wasm_test_function(
            name,
            vec!["xs", "needle"],
            Some(vec![container_type, "Any"]),
            vec![contains.clone(), len.clone()],
        );
        let plan = wasm_representation_plan(&func);

        assert_eq!(
            selected_container_runtime_import(&plan, 0, "contains", &contains),
            contains_import,
            "{name} contains selection drifted"
        );
        assert_eq!(
            selected_container_runtime_import(&plan, 1, "len", &len),
            len_import,
            "{name} len selection drifted"
        );
    }
}

#[test]
fn container_import_selection_uses_manifest_index_store_matrix() {
    let index = wasm_test_op("index", Some("item"), vec!["xs", "key"]);
    let store = wasm_test_op("store_index", None, vec!["xs", "key", "value"]);
    let cases = [
        (
            "typed_dict_index_store",
            "dict",
            Some(WasmRuntimeImport::DictGetitem),
            Some(WasmRuntimeImport::DictSetitem),
        ),
        (
            "typed_tuple_index_store",
            "tuple",
            Some(WasmRuntimeImport::TupleGetitem),
            None,
        ),
        ("typed_list_index_store", "list", None, None),
        ("typed_set_index_store", "set", None, None),
        ("typed_str_index_store", "str", None, None),
    ];

    for (name, container_type, index_import, store_import) in cases {
        let func = wasm_test_function(
            name,
            vec!["xs", "key", "value"],
            Some(vec![container_type, "Any", "Any"]),
            vec![index.clone(), store.clone()],
        );
        let plan = wasm_representation_plan(&func);

        assert_eq!(
            selected_container_runtime_import(&plan, 0, "index", &index),
            index_import,
            "{name} index selection drifted"
        );
        assert_eq!(
            selected_container_runtime_import(&plan, 1, "store_index", &store),
            store_import,
            "{name} store_index selection drifted"
        );
    }
}

#[test]
fn container_import_selection_uses_flat_list_storage_proof() {
    let constants: Vec<_> = [("n", 4), ("i", 0), ("v", 7)]
        .into_iter()
        .map(|(name, value)| {
            let mut op = wasm_test_op("const", Some(name), vec![]);
            op.value = Some(value);
            op
        })
        .collect();
    let make = wasm_test_op("list_int_new", Some("xs"), vec!["n", "v"]);
    let index = wasm_test_op("index", Some("item"), vec!["xs", "i"]);
    let set = wasm_test_op("store_index", None, vec!["xs", "i", "v"]);
    let func = wasm_test_function(
        "flat_list_storage",
        vec![],
        None,
        constants
            .into_iter()
            .chain([make, index.clone(), set.clone()])
            .collect(),
    );
    let plan = wasm_representation_plan(&func);

    assert_eq!(
        selected_container_runtime_import(&plan, 4, "index", &index),
        Some(WasmRuntimeImport::ListIntGetitem)
    );
    assert_eq!(
        selected_container_runtime_import(&plan, 5, "store_index", &set),
        Some(WasmRuntimeImport::ListIntSetitem)
    );
}

#[test]
fn container_import_selection_does_not_trust_annotated_index_protocols() {
    for access in ["index", "store_index"] {
        let mut count = wasm_test_op("const", Some("n"), vec![]);
        count.value = Some(4);
        let mut fill = wasm_test_op("const", Some("v"), vec![]);
        fill.value = Some(7);
        let args = if access == "index" {
            vec!["xs", "i"]
        } else {
            vec!["xs", "i", "v"]
        };
        let op = wasm_test_op(access, Some("result"), args);
        let func = wasm_test_function(
            "annotated_index_protocol",
            vec!["i"],
            Some(vec!["int"]),
            vec![
                count,
                fill,
                wasm_test_op("list_int_new", Some("xs"), vec!["n", "v"]),
                op.clone(),
            ],
        );
        let plan = wasm_representation_plan(&func);
        assert_eq!(
            selected_container_runtime_import(&plan, 3, access, &op),
            None
        );
    }
}
