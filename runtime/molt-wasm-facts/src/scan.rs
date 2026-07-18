use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::Encode;
use wasmparser::{
    CompositeInnerType, ElementItems, ElementKind, Encoding, ExternalKind,
    FuncValidatorAllocations, Operator, OperatorsReader, OperatorsReaderAllocations, Parser,
    Payload, TableInit, TypeRef, ValidPayload, Validator,
};

use crate::encoding::validate_callable_table_attestation;
use crate::layout::decode_callable_table_layout;
use crate::model::*;
use crate::{CALLABLE_TABLE_LAYOUT_SECTION_NAME, CALLABLE_TABLE_SECTION_NAME};

pub fn scan_wasm_link_facts(bytes: &[u8]) -> Result<WasmLinkFacts, String> {
    scan_wasm_link_facts_with_sections(bytes, None)
}

pub(crate) fn scan_wasm_link_facts_with_sections(
    bytes: &[u8],
    mut emit_section: Option<&mut dyn FnMut(u8, &[u8]) -> Result<(), String>>,
) -> Result<WasmLinkFacts, String> {
    let mut function_import_count = 0u32;
    let mut function_import_type_indices = Vec::new();
    let mut defined_function_type_indices = Vec::new();
    let mut function_types = Vec::new();
    let mut declared_function_count = None;
    let mut defined_function_index = 0u32;
    let mut operator_count = 0u64;
    let mut function_references = Vec::new();
    let mut root_function_indices = Vec::new();
    let mut element_function_indices = Vec::new();
    let mut declared_function_indices = Vec::new();
    let mut forbidden_callable_alias_exports = Vec::new();
    let mut dynamic_table_dispatch = false;
    let mut dynamic_dispatch_imports = BTreeSet::new();
    let mut dynamic_dispatch_functions = Vec::new();
    let mut function_reference_dispatch_functions = Vec::new();
    let mut indirect_call_tables = Vec::new();
    let mut indirect_calls = Vec::new();
    let mut table_reads = Vec::new();
    let mut exported_table_indices = Vec::new();
    let mut tables = Vec::new();
    let mut active_element_segments = Vec::new();
    let mut final_active_function_elements: BTreeMap<(u32, u32), Option<u32>> = BTreeMap::new();
    let mut table_mutations = Vec::new();
    let mut callable_table_attestation = None;
    let mut callable_table_layout = None;
    let mut validator = Validator::new();
    let mut validator_allocations = FuncValidatorAllocations::default();
    let mut operator_reader_allocations = OperatorsReaderAllocations::default();
    let mut pending_code_section: Option<(u8, std::ops::Range<usize>, u32)> = None;
    let mut module_header_seen = false;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|error| error.to_string())?;
        let raw_section = payload.as_section();
        let replaced_custom_section = matches!(
            &payload,
            Payload::CustomSection(reader)
                if matches!(
                    reader.name(),
                    CALLABLE_TABLE_SECTION_NAME | CALLABLE_TABLE_LAYOUT_SECTION_NAME
                )
        );
        let deferred_code_section = matches!(&payload, Payload::CodeSectionStart { .. });
        let function_to_validate = match validator
            .payload(&payload)
            .map_err(|error| error.to_string())?
        {
            ValidPayload::Func(function, _) => Some(function),
            _ => None,
        };
        match payload {
            Payload::Version { num, encoding, .. } => {
                if module_header_seen {
                    return Err("duplicate top-level WebAssembly header".to_string());
                }
                module_header_seen = true;
                if num != 1 || encoding != Encoding::Module {
                    return Err(format!(
                        "wasm link facts require a core WebAssembly module version 1, found {encoding:?} version {num}"
                    ));
                }
            }
            Payload::TypeSection(reader) => {
                for rec_group in reader {
                    let rec_group = rec_group.map_err(|error| error.to_string())?;
                    for subtype in rec_group.into_types() {
                        let type_index = u32::try_from(function_types.len())
                            .map_err(|_| "module type index overflow")?;
                        function_types.push(match subtype.composite_type.inner {
                            CompositeInnerType::Func(function_type) => Some(WasmFunctionType {
                                type_index,
                                params: encode_value_types(function_type.params())?,
                                results: encode_value_types(function_type.results())?,
                            }),
                            CompositeInnerType::Array(_)
                            | CompositeInnerType::Struct(_)
                            | CompositeInnerType::Cont(_) => None,
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(|error| error.to_string())?;
                    if let TypeRef::Func(type_index) | TypeRef::FuncExact(type_index) = import.ty {
                        let function_index = function_import_count;
                        function_import_count = function_import_count
                            .checked_add(1)
                            .ok_or("function import count overflow")?;
                        function_import_type_indices.push(type_index);
                        if import.module == "env"
                            && import
                                .name
                                .strip_prefix("molt_call_indirect")
                                .is_some_and(|arity| {
                                    !arity.is_empty()
                                        && arity.bytes().all(|byte| byte.is_ascii_digit())
                                })
                        {
                            dynamic_table_dispatch = true;
                            dynamic_dispatch_imports.insert(function_index);
                        }
                    }
                    if let TypeRef::Table(table_type) = import.ty {
                        tables.push(table_fact(table_type, true, tables.len())?);
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                if declared_function_count.replace(reader.count()).is_some() {
                    return Err("duplicate function section".to_string());
                }
                for type_index in reader {
                    defined_function_type_indices
                        .push(type_index.map_err(|error| error.to_string())?);
                }
            }
            Payload::TableSection(reader) => {
                for table in reader {
                    let table = table.map_err(|error| error.to_string())?;
                    tables.push(table_fact(table.ty, false, tables.len())?);
                    if let TableInit::Expr(expression) = table.init {
                        collect_const_expr_ref_funcs(expression, &mut root_function_indices)?;
                    }
                }
            }
            Payload::GlobalSection(reader) => {
                for global in reader {
                    let global = global.map_err(|error| error.to_string())?;
                    collect_const_expr_ref_funcs(global.init_expr, &mut root_function_indices)?;
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(|error| error.to_string())?;
                    if matches!(export.kind, ExternalKind::Func | ExternalKind::FuncExact) {
                        root_function_indices.push(export.index);
                    }
                    if export.kind == ExternalKind::Table {
                        exported_table_indices.push(export.index);
                    }
                    if export.name.starts_with("__molt_table_ref_") {
                        forbidden_callable_alias_exports.push(export.name.to_string());
                    }
                }
            }
            Payload::StartSection { func, .. } => {
                root_function_indices.push(func);
            }
            Payload::ElementSection(reader) => {
                for element in reader {
                    let element = element.map_err(|error| error.to_string())?;
                    let declared_only =
                        matches!(&element.kind, ElementKind::Passive | ElementKind::Declared);
                    let active = match element.kind {
                        ElementKind::Active {
                            table_index,
                            offset_expr,
                        } => {
                            let mut operators = offset_expr.get_operators_reader();
                            let base = match operators.read().map_err(|error| error.to_string())? {
                                Operator::I32Const { value } if value >= 0 => value as u32,
                                operator => {
                                    return Err(format!(
                                        "unsupported active element offset: {operator:?}"
                                    ));
                                }
                            };
                            if !matches!(
                                operators.read().map_err(|error| error.to_string())?,
                                Operator::End
                            ) || !operators.eof()
                            {
                                return Err("malformed active element offset".to_string());
                            }
                            Some((table_index.unwrap_or(0), base))
                        }
                        ElementKind::Passive | ElementKind::Declared => None,
                    };
                    let mut element_item_count = 0u32;
                    match element.items {
                        ElementItems::Functions(functions) => {
                            for (relative, function_index) in functions.into_iter().enumerate() {
                                element_item_count = element_item_count
                                    .checked_add(1)
                                    .ok_or("element item count overflow")?;
                                let function_index =
                                    function_index.map_err(|error| error.to_string())?;
                                element_function_indices.push(function_index);
                                if declared_only {
                                    declared_function_indices.push(function_index);
                                }
                                if let Some((table_index, base)) = active {
                                    let relative = u32::try_from(relative).map_err(|_| {
                                        "active element offset exceeds u32".to_string()
                                    })?;
                                    let slot = base
                                        .checked_add(relative)
                                        .ok_or("active element callable-table slot overflow")?;
                                    final_active_function_elements
                                        .insert((table_index, slot), Some(function_index));
                                }
                            }
                        }
                        ElementItems::Expressions(_ref_type, expressions) => {
                            for (relative, expression) in expressions.into_iter().enumerate() {
                                element_item_count = element_item_count
                                    .checked_add(1)
                                    .ok_or("element item count overflow")?;
                                let expression = expression.map_err(|error| error.to_string())?;
                                let mut operators = expression.get_operators_reader();
                                let function_index =
                                    match operators.read().map_err(|error| error.to_string())? {
                                        Operator::RefFunc { function_index } => {
                                            element_function_indices.push(function_index);
                                            if declared_only {
                                                declared_function_indices.push(function_index);
                                            }
                                            Some(function_index)
                                        }
                                        Operator::RefNull { .. } => None,
                                        operator => {
                                            return Err(format!(
                                                "unsupported element expression: {operator:?}"
                                            ));
                                        }
                                    };
                                if !matches!(
                                    operators.read().map_err(|error| error.to_string())?,
                                    Operator::End
                                ) || !operators.eof()
                                {
                                    return Err("malformed element expression".to_string());
                                }
                                if let Some((table_index, base)) = active {
                                    let relative = u32::try_from(relative).map_err(|_| {
                                        "active element offset exceeds u32".to_string()
                                    })?;
                                    let slot = base
                                        .checked_add(relative)
                                        .ok_or("active element callable-table slot overflow")?;
                                    final_active_function_elements
                                        .insert((table_index, slot), function_index);
                                }
                            }
                        }
                    }
                    if let Some((table_index, base)) = active {
                        active_element_segments.push(WasmActiveElementSegment {
                            table_index,
                            base,
                            item_count: element_item_count,
                        });
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let function = function_to_validate
                    .ok_or("validator did not provide a function validator for a code body")?;
                let mut function_validator = function.into_validator(validator_allocations);
                let function_index = function_import_count
                    .checked_add(defined_function_index)
                    .ok_or("function index overflow")?;
                defined_function_index = defined_function_index
                    .checked_add(1)
                    .ok_or("defined function count overflow")?;
                let mut direct_calls = Vec::new();
                let mut ref_funcs = Vec::new();
                let mut function_dynamic_dispatch = false;
                let mut function_reference_dispatch = false;
                let mut locals_reader = body.get_binary_reader();
                function_validator
                    .read_locals(&mut locals_reader)
                    .map_err(|error| error.to_string())?;
                let mut operators =
                    OperatorsReader::new_with_allocs(locals_reader, operator_reader_allocations);
                while !operators.eof() {
                    let offset = operators.original_position();
                    let operator = operators.read().map_err(|error| error.to_string())?;
                    function_validator
                        .op(offset, &operator)
                        .map_err(|error| error.to_string())?;
                    operator_count = operator_count
                        .checked_add(1)
                        .ok_or("operator count overflow")?;
                    match operator {
                        Operator::Call { function_index }
                        | Operator::ReturnCall { function_index } => {
                            direct_calls.push(function_index);
                            if dynamic_dispatch_imports.contains(&function_index) {
                                function_dynamic_dispatch = true;
                            }
                        }
                        Operator::RefFunc { function_index } => {
                            ref_funcs.push(function_index);
                        }
                        Operator::CallIndirect { table_index, .. }
                        | Operator::ReturnCallIndirect { table_index, .. } => {
                            dynamic_table_dispatch = true;
                            function_dynamic_dispatch = true;
                            indirect_call_tables.push(table_index);
                            indirect_calls.push(WasmIndirectCall {
                                function_index,
                                table_index,
                            });
                        }
                        Operator::CallRef { .. } | Operator::ReturnCallRef { .. } => {
                            function_reference_dispatch = true;
                        }
                        Operator::TableGet { table } => {
                            table_reads.push(WasmTableRead {
                                function_index,
                                table_index: table,
                            });
                        }
                        Operator::TableSet { table } => record_table_mutation(
                            &mut table_mutations,
                            function_index,
                            "table.set",
                            table,
                            None,
                        ),
                        Operator::TableInit { table, .. } => record_table_mutation(
                            &mut table_mutations,
                            function_index,
                            "table.init",
                            table,
                            None,
                        ),
                        Operator::TableCopy {
                            dst_table,
                            src_table,
                        } => record_table_mutation(
                            &mut table_mutations,
                            function_index,
                            "table.copy",
                            dst_table,
                            Some(src_table),
                        ),
                        Operator::TableGrow { table } => record_table_mutation(
                            &mut table_mutations,
                            function_index,
                            "table.grow",
                            table,
                            None,
                        ),
                        Operator::TableFill { table } => record_table_mutation(
                            &mut table_mutations,
                            function_index,
                            "table.fill",
                            table,
                            None,
                        ),
                        _ => {}
                    }
                }
                operators.finish().map_err(|error| error.to_string())?;
                operator_reader_allocations = operators.into_allocations();
                if function_validator.control_stack_height() != 0 {
                    return Err("control frames remain at end of function body".to_string());
                }
                validator_allocations = function_validator.into_allocations();
                if !direct_calls.is_empty() || !ref_funcs.is_empty() {
                    direct_calls.sort_unstable();
                    direct_calls.dedup();
                    ref_funcs.sort_unstable();
                    ref_funcs.dedup();
                    function_references.push(WasmFunctionReferences {
                        function_index,
                        direct_calls,
                        ref_funcs,
                    });
                }
                if function_dynamic_dispatch {
                    dynamic_dispatch_functions.push(function_index);
                }
                if function_reference_dispatch {
                    function_reference_dispatch_functions.push(function_index);
                }
                if let Some((id, range, remaining)) = pending_code_section.as_mut() {
                    *remaining = remaining
                        .checked_sub(1)
                        .ok_or("code body count exceeds code section declaration")?;
                    if *remaining == 0 {
                        if let Some(emitter) = emit_section.as_mut() {
                            (**emitter)(
                                *id,
                                bytes
                                    .get(range.clone())
                                    .ok_or("wasm code section range exceeds input")?,
                            )?;
                        }
                        pending_code_section = None;
                    }
                }
            }
            Payload::CodeSectionStart { count, range, .. } => {
                if pending_code_section.is_some() {
                    return Err("nested code section publication state".to_string());
                }
                pending_code_section = Some((10, range, count));
                if count == 0 {
                    let (id, range, _) = pending_code_section
                        .take()
                        .ok_or("missing empty code section publication state")?;
                    if let Some(emitter) = emit_section.as_mut() {
                        (**emitter)(
                            id,
                            bytes
                                .get(range)
                                .ok_or("wasm code section range exceeds input")?,
                        )?;
                    }
                }
            }
            Payload::CustomSection(reader) if reader.name() == CALLABLE_TABLE_SECTION_NAME => {
                if callable_table_attestation.is_some() {
                    return Err("duplicate molt.callable_table custom sections".to_string());
                }
                callable_table_attestation = Some(reader.data());
            }
            Payload::CustomSection(reader)
                if reader.name() == CALLABLE_TABLE_LAYOUT_SECTION_NAME =>
            {
                if callable_table_layout.is_some() {
                    return Err("duplicate molt.callable_table.layout custom sections".to_string());
                }
                callable_table_layout = Some(decode_callable_table_layout(reader.data())?);
            }
            _ => {}
        }
        if !replaced_custom_section && !deferred_code_section {
            if let Some((id, range)) = raw_section {
                if let Some(emitter) = emit_section.as_mut() {
                    (**emitter)(
                        id,
                        bytes.get(range).ok_or("wasm section range exceeds input")?,
                    )?;
                }
            }
        }
    }

    if pending_code_section.is_some() {
        return Err("code section ended before all declared bodies were decoded".to_string());
    }
    if !module_header_seen {
        return Err("missing top-level WebAssembly module header".to_string());
    }

    let declared_function_count = declared_function_count.unwrap_or(0);
    if defined_function_index != declared_function_count {
        return Err(format!(
            "function/code section count mismatch: declared {declared_function_count}, decoded {defined_function_index}"
        ));
    }
    let active_function_elements = final_active_function_elements
        .into_iter()
        .filter_map(|((table_index, slot), function_index)| {
            function_index.map(|function_index| WasmActiveFunctionElement {
                table_index,
                slot,
                function_index,
            })
        })
        .collect::<Vec<_>>();
    active_element_segments.sort_by_key(|segment| (segment.table_index, segment.base));
    let function_type_indices = function_import_type_indices
        .into_iter()
        .chain(defined_function_type_indices)
        .collect::<Vec<_>>();
    let mut callable_table_entries = Vec::new();
    for element in active_function_elements
        .iter()
        .filter(|element| element.table_index == 0)
    {
        let function_position = usize::try_from(element.function_index)
            .map_err(|_| "function index exceeds host usize")?;
        let type_index = *function_type_indices
            .get(function_position)
            .ok_or_else(|| {
                format!(
                    "active table slot {} references missing function {}",
                    element.slot, element.function_index
                )
            })?;
        function_types
            .get(usize::try_from(type_index).map_err(|_| "type index exceeds host usize")?)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                format!(
                    "function {} references missing function type {type_index}",
                    element.function_index
                )
            })?;
        callable_table_entries.push(WasmCallableTableEntryFact {
            slot: element.slot,
            function_index: element.function_index,
            type_index,
            role: 0,
        });
    }
    let callable_table_attestation_present = callable_table_attestation.is_some();
    if let Some(attestation) = callable_table_attestation {
        validate_callable_table_attestation(attestation, &callable_table_entries, &function_types)?;
    }
    root_function_indices.sort_unstable();
    root_function_indices.dedup();
    element_function_indices.sort_unstable();
    element_function_indices.dedup();
    declared_function_indices.sort_unstable();
    declared_function_indices.dedup();
    table_mutations.sort_unstable();
    table_mutations.dedup();
    forbidden_callable_alias_exports.sort_unstable();
    forbidden_callable_alias_exports.dedup();
    indirect_call_tables.sort_unstable();
    indirect_call_tables.dedup();
    indirect_calls.sort_unstable();
    indirect_calls.dedup();
    dynamic_dispatch_functions.sort_unstable();
    dynamic_dispatch_functions.dedup();
    function_reference_dispatch_functions.sort_unstable();
    function_reference_dispatch_functions.dedup();
    table_reads.sort_unstable();
    table_reads.dedup();
    exported_table_indices.sort_unstable();
    exported_table_indices.dedup();
    let mut reference_row_by_function = vec![None; function_type_indices.len()];
    for (row_index, row) in function_references.iter().enumerate() {
        if let Some(slot) = reference_row_by_function.get_mut(row.function_index as usize) {
            *slot = Some(row_index);
        }
    }
    let mut reachable_functions = vec![false; function_type_indices.len()];
    let mut worklist = root_function_indices.clone();
    extend_reachable_functions(
        &mut reachable_functions,
        &mut worklist,
        &function_references,
        &reference_row_by_function,
    );
    let reachable_dynamic_dispatch = loop {
        let reachable_dynamic = dynamic_dispatch_functions
            .iter()
            .chain(dynamic_dispatch_imports.iter())
            .any(|function_index| {
                reachable_functions
                    .get(*function_index as usize)
                    .copied()
                    .unwrap_or(false)
            });
        if reachable_dynamic || exported_table_indices.contains(&0) {
            worklist.extend(
                active_function_elements
                    .iter()
                    .filter(|element| element.table_index == 0)
                    .map(|element| element.function_index),
            );
        }
        if table_mutations.iter().any(|mutation| {
            mutation.operation == "table.init"
                && reachable_functions
                    .get(mutation.function_index as usize)
                    .copied()
                    .unwrap_or(false)
        }) {
            worklist.extend(element_function_indices.iter().copied());
        }
        let prior_reachable_count = reachable_functions.iter().filter(|value| **value).count();
        extend_reachable_functions(
            &mut reachable_functions,
            &mut worklist,
            &function_references,
            &reference_row_by_function,
        );
        if reachable_functions.iter().filter(|value| **value).count() == prior_reachable_count {
            break reachable_dynamic;
        }
    };
    let reachable_table_mutations = table_mutations
        .iter()
        .filter(|mutation| {
            reachable_functions
                .get(mutation.function_index as usize)
                .copied()
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    let reachable_function_reference_dispatch =
        function_reference_dispatch_functions
            .iter()
            .any(|function_index| {
                reachable_functions
                    .get(*function_index as usize)
                    .copied()
                    .unwrap_or(false)
            });
    let reachable_table_reads = table_reads
        .iter()
        .filter(|read| {
            reachable_functions
                .get(read.function_index as usize)
                .copied()
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut reachable_indirect_call_tables = indirect_calls
        .iter()
        .filter(|call| {
            reachable_functions
                .get(call.function_index as usize)
                .copied()
                .unwrap_or(false)
        })
        .map(|call| call.table_index)
        .collect::<Vec<_>>();
    reachable_indirect_call_tables.sort_unstable();
    reachable_indirect_call_tables.dedup();
    Ok(WasmLinkFacts {
        schema_version: 3,
        function_import_count,
        defined_function_count: declared_function_count,
        code_body_count: defined_function_index,
        operator_count,
        function_references,
        function_types,
        function_type_indices,
        root_function_indices,
        element_function_indices,
        declared_function_indices,
        active_element_segments,
        active_function_elements,
        callable_table_entries,
        callable_table_attestation_present,
        callable_table_layout,
        table_mutations,
        reachable_table_mutations,
        forbidden_callable_alias_exports,
        dynamic_table_dispatch,
        dynamic_dispatch_functions,
        reachable_dynamic_dispatch,
        function_reference_dispatch_functions,
        reachable_function_reference_dispatch,
        indirect_call_tables,
        reachable_indirect_call_tables,
        indirect_calls,
        table_reads,
        reachable_table_reads,
        exported_table_indices,
        tables,
    })
}

fn extend_reachable_functions(
    reachable: &mut [bool],
    worklist: &mut Vec<u32>,
    references: &[WasmFunctionReferences],
    reference_row_by_function: &[Option<usize>],
) {
    while let Some(function_index) = worklist.pop() {
        let Some(reachable_slot) = reachable.get_mut(function_index as usize) else {
            continue;
        };
        if *reachable_slot {
            continue;
        }
        *reachable_slot = true;
        if let Some(Some(row_index)) = reference_row_by_function.get(function_index as usize) {
            let row = &references[*row_index];
            worklist.extend(row.direct_calls.iter().chain(&row.ref_funcs).copied());
        }
    }
}

fn table_fact(
    table_type: wasmparser::TableType,
    imported: bool,
    table_index: usize,
) -> Result<WasmTableFact, String> {
    let table_index = u32::try_from(table_index).map_err(|_| "table index exceeds u32")?;
    let encoded_element_type = {
        let mut encoded = Vec::new();
        wasm_encoder::RefType::try_from(table_type.element_type)
            .map_err(|error| error.to_string())?
            .encode(&mut encoded);
        encoded
    };
    Ok(WasmTableFact {
        table_index,
        imported,
        minimum: table_type.initial,
        maximum: table_type.maximum,
        table64: table_type.table64,
        shared: table_type.shared,
        untyped_funcref: table_type.element_type == wasmparser::RefType::FUNCREF,
        encoded_element_type,
    })
}

fn encode_value_types(value_types: &[wasmparser::ValType]) -> Result<Vec<Vec<u8>>, String> {
    value_types
        .iter()
        .map(|value_type| {
            let value_type =
                wasm_encoder::ValType::try_from(*value_type).map_err(|error| error.to_string())?;
            let mut encoded = Vec::new();
            value_type.encode(&mut encoded);
            Ok(encoded)
        })
        .collect()
}

fn collect_const_expr_ref_funcs(
    expression: wasmparser::ConstExpr<'_>,
    declared_or_referenced_functions: &mut Vec<u32>,
) -> Result<(), String> {
    let mut operators = expression.get_operators_reader();
    while !operators.eof() {
        if let Operator::RefFunc { function_index } =
            operators.read().map_err(|error| error.to_string())?
        {
            declared_or_referenced_functions.push(function_index);
        }
    }
    operators.finish().map_err(|error| error.to_string())
}

fn record_table_mutation(
    mutations: &mut Vec<WasmTableMutation>,
    function_index: u32,
    operation: &'static str,
    table_index: u32,
    source_table_index: Option<u32>,
) {
    mutations.push(WasmTableMutation {
        function_index,
        operation,
        table_index,
        source_table_index,
    });
}
