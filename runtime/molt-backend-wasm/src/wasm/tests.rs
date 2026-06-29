use super::container_runtime_select::selected_container_runtime_import;
use super::{WasmBackend, WasmCompileOptions, WasmProfile};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::lir_fast::is_production_lir_wasm_fast_path_name;
use crate::wasm_abi::{
    CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY, WasmRuntimeImport, poll_table_imports,
    wasm_runtime_export_name, wasm_runtime_import,
};
use crate::wasm_plan::{
    is_shared_drop_fact_marker, wasm_scalar_integer_fast_path_for_op,
    wasm_scalar_truthiness_fast_path_for_name,
};
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

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

#[test]
fn lir_fast_literal_const_materialization_emits_valid_wasm() {
    let literal_bytes = b"hello wasm literal";
    let mut literal = wasm_test_op("const_str", Some("literal"), vec![]);
    literal.s_value = Some(String::from_utf8(literal_bytes.to_vec()).expect("ascii literal"));
    let mut ret = wasm_test_op("ret", None, vec!["literal"]);
    ret.var = Some("literal".to_string());
    let func = wasm_test_function(
        "m____molt_globals_builtin__literal_const",
        vec![],
        None,
        vec![literal, ret],
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
        .expect("LIR-fast literal const materialization must emit valid WASM");

    let imports = wasm_function_import_indices(&wasm);
    let string_from_bytes = *imports
        .get("string_from_bytes")
        .unwrap_or_else(|| panic!("string_from_bytes import missing; imports={imports:?}"));
    assert!(
        wasm_direct_call_indices(&wasm).contains(&string_from_bytes),
        "LIR-fast materialization must emit a direct call to string_from_bytes"
    );
    assert!(
        wasm_data_segment_payloads(&wasm)
            .iter()
            .any(|payload| payload.as_slice() == literal_bytes),
        "LIR-fast materialization must write literal bytes into a data segment"
    );
}

#[test]
fn generic_attr_ic_uses_transported_source_op_idx() {
    let source_op_idx = 17;
    let mut load_attr = wasm_test_op("get_attr_generic_obj", Some("value"), vec!["obj"]);
    load_attr.s_value = Some("field".to_string());
    load_attr.source_op_idx = Some(source_op_idx);
    let mut ret = wasm_test_op("ret", None, vec!["value"]);
    ret.var = Some("value".to_string());
    let func = wasm_test_function("generic_attr_ic", vec!["obj"], None, vec![load_attr, ret]);
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
        .expect("generic attr IC lowering must emit valid WASM");

    let expected_site_bits = molt_codegen_abi::box_int_bits(molt_codegen_abi::stable_ic_site_id(
        "generic_attr_ic",
        source_op_idx as usize,
        "get_attr_generic_obj",
    ));
    assert!(
        wasm_i64_consts(&wasm).contains(&expected_site_bits),
        "generic WASM attr IC must use transported source_op_idx for site id"
    );

    let imports = wasm_function_import_indices(&wasm);
    let get_attr_object_ic = *imports
        .get("get_attr_object_ic")
        .unwrap_or_else(|| panic!("get_attr_object_ic import missing; imports={imports:?}"));
    assert!(
        wasm_direct_call_indices(&wasm).contains(&get_attr_object_ic),
        "generic WASM attr IC must call get_attr_object_ic"
    );
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

fn wasm_object_new_bound_ir(payload_size: Option<i64>) -> SimpleIR {
    let mut allocate = wasm_test_op("object_new_bound", Some("obj"), vec!["cls"]);
    allocate.value = payload_size;
    let mut ret = wasm_test_op("ret", None, vec!["obj"]);
    ret.var = Some("obj".to_string());
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec!["cls"],
            None,
            vec![allocate, ret],
        )],
        profile: None,
    }
}

fn wasm_method_ic_ir(kind: &str, extra_arg_count: usize) -> SimpleIR {
    let mut args = match kind {
        "call_method_ic" => vec!["recv".to_string()],
        "call_super_method_ic" => vec!["cls".to_string(), "self_obj".to_string()],
        _ => panic!("unsupported method IC kind {kind}"),
    };
    for idx in 0..extra_arg_count {
        args.push(format!("arg{idx}"));
    }
    let call = OpIR {
        kind: kind.to_string(),
        out: Some("out".to_string()),
        args: Some(args.clone()),
        s_value: Some("selected_method".to_string()),
        ..OpIR::default()
    };
    let mut ret = wasm_test_op("ret", None, vec!["out"]);
    ret.var = Some("out".to_string());
    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: args,
            ops: vec![call, ret],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
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
    let plan = ScalarRepresentationPlan::for_function_ir(&func);

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
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

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
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

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
        selected_container_runtime_import(&plan, 1, "index", &index),
        Some(WasmRuntimeImport::ListIntGetitem)
    );
    assert_eq!(
        selected_container_runtime_import(&plan, 2, "store_index", &set),
        Some(WasmRuntimeImport::ListIntSetitem)
    );
}

#[test]
fn object_new_bound_import_demand_and_codegen_follow_payload_selector() {
    let cases = [
        (
            "object_new_bound_unsized_selector",
            None,
            "object_new_bound",
            "object_new_bound_sized",
        ),
        (
            "object_new_bound_sized_selector",
            Some(24),
            "object_new_bound_sized",
            "object_new_bound",
        ),
    ];
    for (name, payload_size, expected_import, rejected_import) in cases {
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_object_new_bound_ir(payload_size));
        let reloc_imports = wasm_function_import_names(&reloc_wasm);
        assert!(
            reloc_imports.iter().any(|name| name == expected_import),
            "{expected_import} must be selected by Auto+reloc import demand; imports={reloc_imports:?}"
        );

        let direct_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_object_new_bound_ir(payload_size));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        let expected_index = *import_indices
            .get(expected_import)
            .unwrap_or_else(|| panic!("{expected_import} import must exist"));
        assert!(
            call_indices.contains(&expected_index),
            "{name} must directly call {expected_import} from molt_main; calls={call_indices:?}"
        );
        if let Some(rejected_index) = import_indices.get(rejected_import) {
            assert!(
                !call_indices.contains(rejected_index),
                "{name} must not directly call {rejected_import} from molt_main; calls={call_indices:?}"
            );
        }
        if let Some(size) = payload_size {
            assert!(
                wasm_i64_consts(&direct_wasm).contains(&size),
                "{name} must emit payload byte size {size}"
            );
        }
    }
}

#[test]
fn method_ic_import_demand_and_codegen_follow_generated_arity_selector() {
    let cases = [
        (
            "call_method_ic",
            0,
            "call_method_ic0",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_method_ic",
            2,
            "call_method_ic2",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_method_ic",
            5,
            "call_method_ic4",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            0,
            "call_super_method_ic0",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            2,
            "call_super_method_ic2",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            5,
            "call_super_method_ic4",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
    ];
    for (kind, extra_arg_count, expected_import, family_imports) in cases {
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_method_ic_ir(kind, extra_arg_count));
        let reloc_imports = wasm_function_import_names(&reloc_wasm);
        assert!(
            reloc_imports.iter().any(|name| name == expected_import),
            "{kind}/{extra_arg_count} must retain selected import {expected_import}; imports={reloc_imports:?}"
        );
        for import_name in family_imports {
            if import_name != expected_import {
                assert!(
                    !reloc_imports.iter().any(|name| name == import_name),
                    "{kind}/{extra_arg_count} must not retain unselected import {import_name}; imports={reloc_imports:?}"
                );
            }
        }

        let direct_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_method_ic_ir(kind, extra_arg_count));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        let expected_index = *import_indices
            .get(expected_import)
            .unwrap_or_else(|| panic!("{expected_import} import must exist"));
        assert!(
            call_indices.contains(&expected_index),
            "{kind}/{extra_arg_count} must directly call {expected_import}; calls={call_indices:?}"
        );
        for import_name in family_imports {
            if let Some(unselected_index) = import_indices.get(import_name)
                && import_name != expected_import
            {
                assert!(
                    !call_indices.contains(unselected_index),
                    "{kind}/{extra_arg_count} must not directly call {import_name}; calls={call_indices:?}"
                );
            }
        }
        assert!(
            wasm_data_segment_payloads(&direct_wasm)
                .iter()
                .any(|payload| payload.as_slice() == b"selected_method"),
            "{kind}/{extra_arg_count} must materialize the selected method name"
        );
    }
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

fn wasm_function_import_indices(wasm: &[u8]) -> BTreeMap<String, u32> {
    let mut imports = BTreeMap::new();
    let mut func_index = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            for import in reader.into_imports().flatten() {
                if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                    imports.insert(import.name.to_string(), func_index);
                    func_index += 1;
                }
            }
        }
    }
    imports
}

fn wasm_direct_call_indices(wasm: &[u8]) -> Vec<u32> {
    wasm_direct_call_indices_for_body(wasm, None)
}

fn wasm_direct_call_indices_for_export(wasm: &[u8], export_name: &str) -> Vec<u32> {
    let export_index = *wasm_function_export_indices(wasm)
        .get(export_name)
        .unwrap_or_else(|| panic!("missing function export {export_name}"));
    let import_count = wasm_function_import_indices(wasm).len() as u32;
    let body_index = export_index
        .checked_sub(import_count)
        .unwrap_or_else(|| panic!("export {export_name} is an import, not a defined function"));
    wasm_direct_call_indices_for_body(wasm, Some(body_index))
}

fn wasm_direct_call_indices_for_body(wasm: &[u8], body_filter: Option<u32>) -> Vec<u32> {
    let mut calls = Vec::new();
    let mut body_index = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::CodeSectionEntry(body)) = payload
            && let Ok(mut ops) = body.get_operators_reader()
        {
            if body_filter.is_some_and(|target| target != body_index) {
                body_index += 1;
                continue;
            }
            while let Ok(op) = ops.read() {
                if let wasmparser::Operator::Call { function_index } = op {
                    calls.push(function_index);
                }
            }
            body_index += 1;
        }
    }
    if let Some(target) = body_filter {
        assert!(
            target < body_index,
            "requested WASM body {target}, but module only has {body_index} code bodies"
        );
    }
    calls
}

fn wasm_i64_consts(wasm: &[u8]) -> Vec<i64> {
    let mut values = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::CodeSectionEntry(body)) = payload
            && let Ok(mut ops) = body.get_operators_reader()
        {
            while let Ok(op) = ops.read() {
                if let wasmparser::Operator::I64Const { value } = op {
                    values.push(value);
                }
            }
        }
    }
    values
}

fn wasm_data_segment_payloads(wasm: &[u8]) -> Vec<Vec<u8>> {
    let mut payloads = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::DataSection(reader)) = payload {
            for data in reader.into_iter().flatten() {
                payloads.push(data.data.to_vec());
            }
        }
    }
    payloads
}

fn wasm_element_function_indices(wasm: &[u8]) -> Vec<u32> {
    use wasmparser::ElementItems;

    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ElementSection(reader)) = payload {
            for element in reader.into_iter().flatten() {
                if let ElementItems::Functions(funcs) = element.items {
                    return funcs
                        .into_iter_with_offsets()
                        .flatten()
                        .map(|(_offset, func_index)| func_index)
                        .collect();
                }
            }
        }
    }
    panic!("expected active function element section");
}

fn wasm_function_section_type_indices(wasm: &[u8]) -> Vec<u32> {
    let mut type_indices = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::FunctionSection(reader)) = payload {
            type_indices.extend(reader.into_iter().flatten());
        }
    }
    type_indices
}

fn wasm_function_exports(wasm: &[u8]) -> BTreeSet<String> {
    let mut exports = BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ExportSection(reader)) = payload {
            for export in reader.into_iter().flatten() {
                if export.kind == ExternalKind::Func {
                    exports.insert(export.name.to_string());
                }
            }
        }
    }
    exports
}

fn wasm_function_export_indices(wasm: &[u8]) -> BTreeMap<String, u32> {
    let mut exports = BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ExportSection(reader)) = payload {
            for export in reader.into_iter().flatten() {
                if export.kind == ExternalKind::Func {
                    exports.insert(export.name.to_string(), export.index);
                }
            }
        }
    }
    exports
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
fn call_indirect_exports_follow_manifest_imports() {
    let func = wasm_test_function(
        "call_indirect_exports",
        vec![],
        None,
        vec![wasm_test_op("ret_void", None, vec![])],
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
        .expect("call_indirect manifest export module must be structurally valid WASM");

    let exported_call_indirects: BTreeSet<String> = wasm_function_exports(&wasm)
        .into_iter()
        .filter(|name| name.starts_with("molt_call_indirect"))
        .collect();
    let manifest_call_indirects: BTreeSet<String> = CALL_INDIRECT_IMPORTS
        .iter()
        .map(|spec| spec.import_name.to_string())
        .collect();

    assert_eq!(exported_call_indirects, manifest_call_indirects);
    assert_eq!(
        CALL_INDIRECT_MAX_ARITY,
        CALL_INDIRECT_IMPORTS
            .last()
            .expect("generated call_indirect import family must be non-empty")
            .arity
    );
}

#[test]
fn call_indirect_type_layout_and_sentinel_table_slot_are_pinned() {
    let func = wasm_test_function(
        "call_indirect_type_layout",
        vec![],
        None,
        vec![wasm_test_op("ret_void", None, vec![])],
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
        .expect("call_indirect type-layout module must be structurally valid WASM");

    let import_count = wasm_function_import_indices(&wasm).len() as u32;
    let function_type_indices = wasm_function_section_type_indices(&wasm);
    let export_indices = wasm_function_export_indices(&wasm);
    let signatures = wasm_type_section_signatures(&wasm);

    let first_call_indirect_import = CALL_INDIRECT_IMPORTS
        .first()
        .expect("generated call_indirect import family must be non-empty");
    let first_call_indirect_idx = *export_indices
        .get(first_call_indirect_import.import_name)
        .expect("first call_indirect export must exist");
    for (offset, spec) in CALL_INDIRECT_IMPORTS.iter().enumerate() {
        let func_idx = *export_indices
            .get(spec.import_name)
            .unwrap_or_else(|| panic!("{} export must exist", spec.import_name));
        assert_eq!(
            func_idx,
            first_call_indirect_idx + offset as u32,
            "{} export must stay in generated call_indirect order",
            spec.import_name
        );
        let type_idx = function_type_indices[(func_idx - import_count) as usize];
        assert_eq!(
            signatures[type_idx as usize],
            (spec.arity + 1, 1),
            "{} wrapper must accept table index plus {} args and return one value",
            spec.import_name,
            spec.arity
        );
    }

    let sentinel_func_idx = first_call_indirect_idx + CALL_INDIRECT_IMPORTS.len() as u32;
    let element_indices = wasm_element_function_indices(&wasm);
    let poll_table_prefix = poll_table_imports()
        .filter_map(|spec| spec.poll_table_slot)
        .max()
        .unwrap_or(0) as usize
        + 1;
    let occupied_poll_slots: BTreeSet<usize> = poll_table_imports()
        .filter_map(|spec| spec.poll_table_slot.map(|slot| slot as usize))
        .collect();
    for slot in 0..poll_table_prefix {
        if !occupied_poll_slots.contains(&slot) {
            assert_eq!(
                element_indices[slot], sentinel_func_idx,
                "unassigned poll-table slot {slot} must point at the generated sentinel"
            );
        }
    }
}

#[test]
fn poll_table_slots_follow_manifest_slot_numbers() {
    let func = wasm_test_function(
        "slot_layout",
        vec![],
        None,
        vec![wasm_test_op("ret_void", None, vec![])],
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
        .expect("poll table layout test module must be structurally valid WASM");

    let import_indices = wasm_function_import_indices(&wasm);
    let element_indices = wasm_element_function_indices(&wasm);
    for import in [
        WasmRuntimeImport::AsyncSleepPoll,
        WasmRuntimeImport::PromisePoll,
        WasmRuntimeImport::ContextlibAsyncExitstackEnterContextPoll,
    ] {
        let import_name = import.name();
        let slot = crate::wasm_abi::IMPORT_REGISTRY[import as usize]
            .poll_table_slot
            .unwrap_or_else(|| panic!("missing generated poll slot for {import_name}"));
        let func_index = *import_indices
            .get(import_name)
            .unwrap_or_else(|| panic!("missing poll import {import_name}"));
        assert_eq!(
            element_indices[slot as usize], func_index,
            "poll import {import_name} must occupy manifest table slot {slot}"
        );
    }
}

#[test]
fn runtime_import_aliases_follow_manifest_runtime_names() {
    assert_eq!(
        wasm_runtime_import("importlib_import_transaction"),
        Some(WasmRuntimeImport::ImportlibImportTransaction)
    );
    assert_eq!(
        wasm_runtime_import("molt_importlib_import_transaction"),
        Some(WasmRuntimeImport::ImportlibImportTransaction)
    );
    assert_eq!(
        wasm_runtime_import("socket_drop"),
        Some(WasmRuntimeImport::SocketDrop)
    );
    assert_eq!(
        wasm_runtime_import("molt_socket_drop"),
        Some(WasmRuntimeImport::SocketDrop)
    );
    assert_eq!(wasm_runtime_import("molt_alloc"), None);
    assert_eq!(
        WasmRuntimeImport::ImportlibImportTransaction.runtime_export_name(),
        "molt_importlib_import_transaction"
    );
    assert_eq!(
        wasm_runtime_export_name("importlib_import_transaction"),
        Some("molt_importlib_import_transaction")
    );
    assert_eq!(
        wasm_runtime_export_name("molt_importlib_import_transaction"),
        Some("molt_importlib_import_transaction")
    );
    assert_eq!(
        wasm_runtime_export_name("socket_drop"),
        Some("molt_socket_drop")
    );
    assert_eq!(wasm_runtime_export_name("molt_alloc"), None);
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
fn void_runtime_callable_wrapper_uses_manifest_result_type() {
    let mut socket_drop = wasm_test_op("builtin_func", Some("fn"), vec![]);
    socket_drop.s_value = Some("molt_socket_drop".to_string());
    socket_drop.value = Some(1);
    let func = wasm_test_function(
        "socket_drop_callable",
        vec![],
        None,
        vec![socket_drop, wasm_test_op("ret_void", None, vec![])],
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
        .expect("void callable wrapper must synthesize None after the runtime import call");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("socket_drop")
        .expect("socket_drop runtime import must be registered");
    assert_eq!(
        sigs[import_type as usize],
        (1, 0),
        "socket_drop import ABI must be manifest void, not locally defaulted to i64"
    );
}

#[test]
fn intrinsic_runtime_callables_are_manifest_backed() {
    let mut load_intrinsic = wasm_test_op("builtin_func", Some("fn"), vec![]);
    load_intrinsic.s_value = Some("molt_load_intrinsic_runtime".to_string());
    load_intrinsic.value = Some(2);
    let func = wasm_test_function(
        "load_intrinsic_callable",
        vec![],
        None,
        vec![load_intrinsic, wasm_test_op("ret_void", None, vec![])],
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
        .expect("intrinsic resolver callable must compile through generated ABI metadata");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("load_intrinsic_runtime")
        .expect("load_intrinsic_runtime import must be manifest-backed");
    assert_eq!(sigs[import_type as usize], (2, 1));
}

#[test]
fn gpu_context_runtime_ops_are_manifest_backed() {
    let mut ret = wasm_test_op("ret", None, vec!["tid"]);
    ret.var = Some("tid".to_string());
    let func = wasm_test_function(
        "gpu_context_runtime_ops",
        vec![],
        None,
        vec![
            wasm_test_op("gpu_thread_id", Some("tid"), vec![]),
            wasm_test_op("gpu_block_id", Some("bid"), vec![]),
            wasm_test_op("gpu_block_dim", Some("bdim"), vec![]),
            wasm_test_op("gpu_grid_dim", Some("gdim"), vec![]),
            wasm_test_op("gpu_barrier", Some("barrier"), vec![]),
            ret,
        ],
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
        .expect("GPU context runtime ops must compile through generated ABI metadata");

    let import_types = wasm_function_import_type_indices(&wasm);
    let import_indices = wasm_function_import_indices(&wasm);
    let call_indices = wasm_direct_call_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    for import_name in [
        "gpu_thread_id",
        "gpu_block_id",
        "gpu_block_dim",
        "gpu_grid_dim",
        "gpu_barrier",
    ] {
        let import_type = *import_types
            .get(import_name)
            .unwrap_or_else(|| panic!("{import_name} import must be manifest-backed"));
        assert_eq!(
            sigs[import_type as usize],
            (0, 1),
            "{import_name} must use the manifest [] -> i64 ABI"
        );
        let import_index = *import_indices
            .get(import_name)
            .unwrap_or_else(|| panic!("{import_name} import must stay live"));
        assert!(
            call_indices.contains(&import_index),
            "{import_name} must be emitted as a direct runtime call"
        );
    }
}

#[test]
#[should_panic(expected = "builtin runtime callable arity mismatch")]
fn builtin_callable_observed_arity_must_match_manifest() {
    let mut import_transaction = wasm_test_op("builtin_func", Some("fn"), vec![]);
    import_transaction.s_value = Some("molt_importlib_import_transaction".to_string());
    import_transaction.value = Some(4);
    let func = wasm_test_function(
        "stale_callable_arity",
        vec![],
        None,
        vec![import_transaction, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
}

#[test]
#[should_panic(expected = "direct runtime call missing WASM ABI manifest import")]
fn direct_molt_runtime_call_without_manifest_import_fails_closed() {
    let mut call = wasm_test_op("call", Some("out"), vec!["arg"]);
    call.s_value = Some("molt_unregistered_runtime_probe".to_string());
    let func = wasm_test_function(
        "unknown_direct_runtime_call",
        vec!["arg"],
        None,
        vec![call, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
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
