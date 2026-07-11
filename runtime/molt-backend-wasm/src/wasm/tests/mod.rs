use super::container_runtime_select::selected_container_runtime_import;
use super::{WasmBackend, WasmCompileOptions, WasmCompileOutput, WasmProfile};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::lir_fast::is_production_lir_wasm_fast_path_name;
use crate::wasm_abi::{
    CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY, POLL_TABLE_IMPORTS,
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS,
    ReservedRuntimeCallableDispatch, WasmRuntimeImport, wasm_runtime_export_name,
    wasm_runtime_import,
};
use crate::wasm_options::RELOC_TABLE_BASE_DEFAULT;
use crate::wasm_plan::{
    detect_multi_return_candidates, is_shared_drop_fact_marker,
    wasm_scalar_integer_fast_path_for_op, wasm_scalar_truthiness_fast_path_for_name,
};
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

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

fn wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir: SimpleIR) -> WasmCompileOutput {
    let multi_return_candidates = detect_multi_return_candidates(&ir);
    let trampoline_analysis =
        super::trampoline_analysis::analyze_wasm_trampolines(&ir, multi_return_candidates);
    WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .emit_wasm_module(ir, BTreeMap::new(), trampoline_analysis)
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

fn wasm_native_callable_ir(abi: &str) -> SimpleIR {
    wasm_native_callable_ir_with_args(abi, vec!["arg"])
}

fn wasm_native_callable_ir_with_args(abi: &str, args: Vec<&str>) -> SimpleIR {
    let mut native_call = wasm_test_op("invoke_ffi", Some("out"), args.clone());
    native_call.native_callable_export =
        Some("nativepkg.ndimage.distance_transform_edt".to_string());
    native_call.native_callable_binding = Some("direct_symbol".to_string());
    native_call.native_callable_symbol =
        Some("molt_nativepkg_ndimage_distance_transform_edt".to_string());
    native_call.native_callable_abi = Some(abi.to_string());
    let mut ret = wasm_test_op("ret", None, vec!["out"]);
    ret.var = Some("out".to_string());
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            args,
            None,
            vec![native_call, ret],
        )],
        profile: None,
    }
}

fn wasm_module_attr_native_callable_ir(abi: &str, args: Vec<&str>) -> SimpleIR {
    let mut native_call = wasm_test_op("invoke_ffi", Some("out"), args.clone());
    native_call.native_callable_export =
        Some("nativepkg.ndimage.distance_transform_edt".to_string());
    native_call.native_callable_binding = Some("module_attr".to_string());
    native_call.native_callable_abi = Some(abi.to_string());
    let mut ret = wasm_test_op("ret", None, vec!["out"]);
    ret.var = Some("out".to_string());
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            args,
            None,
            vec![native_call, ret],
        )],
        profile: None,
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

fn wasm_function_import_modules(wasm: &[u8]) -> BTreeMap<String, String> {
    let mut imports = BTreeMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            for import in reader.into_imports().flatten() {
                if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                    imports.insert(import.name.to_string(), import.module.to_string());
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

fn wasm_operator_debug_for_export(wasm: &[u8], export_name: &str) -> Vec<String> {
    let export_index = *wasm_function_export_indices(wasm)
        .get(export_name)
        .unwrap_or_else(|| panic!("missing function export {export_name}"));
    let import_count = wasm_function_import_indices(wasm).len() as u32;
    let body_filter = export_index
        .checked_sub(import_count)
        .unwrap_or_else(|| panic!("export {export_name} is an import, not a defined function"));
    let mut body_index = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::CodeSectionEntry(body)) = payload
            && let Ok(mut ops) = body.get_operators_reader()
        {
            if body_filter != body_index {
                body_index += 1;
                continue;
            }
            let mut out = Vec::new();
            while let Ok(op) = ops.read() {
                out.push(format!("{op:?}"));
            }
            return out;
        }
    }
    panic!("requested WASM body {body_filter}, but no matching code body was found")
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

fn wasm_table_set_refs_for_export(wasm: &[u8], export_name: &str) -> BTreeMap<i32, u32> {
    let export_index = *wasm_function_export_indices(wasm)
        .get(export_name)
        .unwrap_or_else(|| panic!("missing function export {export_name}"));
    let import_count = wasm_function_import_indices(wasm).len() as u32;
    let body_filter = export_index
        .checked_sub(import_count)
        .unwrap_or_else(|| panic!("export {export_name} is an import, not a defined function"));
    let mut refs = BTreeMap::new();
    let mut body_index = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::CodeSectionEntry(body)) = payload
            && let Ok(mut ops) = body.get_operators_reader()
        {
            if body_filter != body_index {
                body_index += 1;
                continue;
            }
            let mut slot = None;
            let mut func = None;
            while let Ok(op) = ops.read() {
                match op {
                    wasmparser::Operator::I32Const { value } => {
                        slot = Some(value);
                    }
                    wasmparser::Operator::RefFunc { function_index } => {
                        func = Some(function_index);
                    }
                    wasmparser::Operator::TableSet { .. } => {
                        if let (Some(slot), Some(func)) = (slot.take(), func.take()) {
                            refs.insert(slot, func);
                        }
                    }
                    _ => {}
                }
            }
            return refs;
        }
    }
    panic!("requested WASM body {body_filter}, but no matching code body was found")
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

fn wasm_type_section_value_signatures(wasm: &[u8]) -> Vec<(Vec<String>, Vec<String>)> {
    use wasmparser::CompositeInnerType;
    let mut sigs = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Ok(Payload::TypeSection(reader)) = payload {
            for rec_group in reader.into_iter() {
                let rec_group = rec_group.expect("valid rec group");
                for sub_type in rec_group.into_types() {
                    if let CompositeInnerType::Func(f) = &sub_type.composite_type.inner {
                        sigs.push((
                            f.params()
                                .iter()
                                .map(|value| format!("{value:?}"))
                                .collect(),
                            f.results()
                                .iter()
                                .map(|value| format!("{value:?}"))
                                .collect(),
                        ));
                    }
                }
            }
        }
    }
    sigs
}

mod call_indirect_table;
mod container_scalar;
mod exception_eh;
mod import_codegen;
mod native_callable;
mod runtime_callable;
mod task_trampoline;
