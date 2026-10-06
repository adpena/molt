use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::Encode;
use wasmparser::{
    BinaryReader, CompositeInnerType, ElementItems, ElementKind, Encoding, ExternalKind,
    FuncValidatorAllocations, KnownCustom, Linking, LinkingSectionReader, Operator,
    OperatorsReader, OperatorsReaderAllocations, Parser, Payload, RelocSectionReader,
    RelocationType, SymbolFlags, SymbolInfo, TableInit, TypeRef, ValidPayload, Validator,
};

use crate::encoding::validate_callable_table_attestation;
use crate::layout::decode_callable_table_layout;
use crate::model::*;
use crate::{
    CALLABLE_TABLE_LAYOUT_SECTION_NAME, CALLABLE_TABLE_SECTION_NAME, WASM_LINK_FACTS_SCHEMA_VERSION,
};

pub fn scan_wasm_link_facts(bytes: &[u8]) -> Result<WasmLinkFacts, String> {
    scan_wasm_link_facts_with_sections(bytes, None)
}

type SectionEmitter<'a> = dyn FnMut(u8, &[u8]) -> Result<(), String> + 'a;

fn section_bytes(bytes: &[u8], range: std::ops::Range<u64>) -> Result<&[u8], &'static str> {
    // Parser positions are format offsets, independent of the host pointer width.
    // Narrow only when borrowing from the actual input buffer, without truncation.
    let start =
        usize::try_from(range.start).map_err(|_| "wasm section offset exceeds address space")?;
    let end =
        usize::try_from(range.end).map_err(|_| "wasm section offset exceeds address space")?;
    bytes
        .get(start..end)
        .ok_or("wasm section range exceeds input")
}

#[cfg(test)]
mod section_range_tests {
    use super::section_bytes;

    #[test]
    fn parser_offsets_are_checked_at_the_input_boundary() {
        let bytes = [10, 20, 30, 40];
        assert_eq!(section_bytes(&bytes, 1..3), Ok(&bytes[1..3]));
        assert_eq!(section_bytes(&bytes, 4..4), Ok(&bytes[4..4]));
        for (start, end) in [
            (0, 5),
            (3, 1),
            (1 << 32, (1 << 32) + 2),
            (u64::MAX, u64::MAX),
        ] {
            assert!(section_bytes(&bytes, start..end).is_err(), "{start}..{end}");
        }
    }
}

const SPLIT_RUNTIME_GOT_DATA_PREFIX: &str = "GOT.data.internal.";

#[derive(Clone, Copy)]
struct DefinedGlobalShape {
    initial_address: Option<u32>,
}

#[derive(Clone, Copy)]
enum PendingExportKind {
    Function { exact: bool },
    Global,
    Memory,
    Table,
    Tag,
}

impl PendingExportKind {
    const fn external_kind(self) -> u8 {
        match self {
            Self::Function { .. } => 0,
            Self::Table => 1,
            Self::Memory => 2,
            Self::Global => 3,
            Self::Tag => 4,
        }
    }
}

struct PendingExport {
    name: String,
    kind: PendingExportKind,
    index: u32,
}

#[derive(Clone)]
struct LinkingDataSymbol {
    name: String,
    flags: SymbolFlags,
    defined: bool,
}

fn canonical_function_type(
    type_index: u32,
    exact: bool,
    function_types: &[Option<WasmFunctionType>],
) -> Result<WasmCanonicalExternType, String> {
    let function_type = function_types
        .get(type_index as usize)
        .and_then(Option::as_ref)
        .ok_or_else(|| format!("function references non-function type index {type_index}"))?;
    Ok(WasmCanonicalExternType::Function {
        exact,
        params: function_type.params.clone(),
        results: function_type.results.clone(),
    })
}

fn canonical_global_type(
    global_type: wasmparser::GlobalType,
) -> Result<WasmCanonicalExternType, String> {
    let mut encoded = encode_value_types(&[global_type.content_type])?;
    Ok(WasmCanonicalExternType::Global {
        value_type: encoded.pop().ok_or("global value type encoding is empty")?,
        mutable: global_type.mutable,
        shared: global_type.shared,
    })
}

fn canonical_memory_type(memory_type: wasmparser::MemoryType) -> WasmCanonicalExternType {
    WasmCanonicalExternType::Memory {
        memory64: memory_type.memory64,
        shared: memory_type.shared,
        minimum: memory_type.initial,
        maximum: memory_type.maximum,
        page_size_log2: memory_type.page_size_log2,
    }
}

fn canonical_table_type(table: &WasmTableFact) -> WasmCanonicalExternType {
    WasmCanonicalExternType::Table {
        table64: table.table64,
        shared: table.shared,
        minimum: table.minimum,
        maximum: table.maximum,
        element_type: table.encoded_element_type.clone(),
    }
}

fn canonical_tag_type(
    tag_type: wasmparser::TagType,
    function_types: &[Option<WasmFunctionType>],
) -> Result<WasmCanonicalExternType, String> {
    let function_type = function_types
        .get(tag_type.func_type_idx as usize)
        .and_then(Option::as_ref)
        .ok_or_else(|| {
            format!(
                "tag references non-function type index {}",
                tag_type.func_type_idx
            )
        })?;
    Ok(WasmCanonicalExternType::Tag {
        tag_kind: match tag_type.kind {
            wasmparser::TagKind::Exception => "exception".to_string(),
        },
        params: function_type.params.clone(),
        results: function_type.results.clone(),
    })
}

#[derive(Clone, Copy)]
struct GotSymbolBinding {
    global_index: u32,
    flags: SymbolFlags,
    defined: bool,
}

fn insert_got_global(
    symbol: &str,
    global_index: u32,
    flags: SymbolFlags,
    defined: bool,
    got_symbols: &mut BTreeMap<String, GotSymbolBinding>,
    got_indices: &mut BTreeSet<u32>,
) -> Result<(), String> {
    if let Some(previous) = got_symbols.get_mut(symbol) {
        if previous.global_index != global_index {
            return Err(format!("duplicate split-runtime GOT data symbol: {symbol}"));
        }
        previous.flags |= flags;
        previous.defined &= defined;
        return Ok(());
    }
    got_symbols.insert(
        symbol.to_string(),
        GotSymbolBinding {
            global_index,
            flags,
            defined,
        },
    );
    if !got_indices.insert(global_index) {
        return Err(format!(
            "duplicate split-runtime GOT data global index: {global_index}"
        ));
    }
    Ok(())
}

fn linking_symbol_fact(
    symbol: SymbolInfo<'_>,
    symbol_index: u32,
    imports: &[WasmCanonicalImportTypeFact],
) -> Result<Option<WasmLinkingSymbolFact>, String> {
    let (kind, external_kind, flags, index, name, data) = match symbol {
        SymbolInfo::Func { flags, index, name } => ("function", 0, flags, Some(index), name, None),
        SymbolInfo::Global { flags, index, name } => ("global", 3, flags, Some(index), name, None),
        SymbolInfo::Table { flags, index, name } => ("table", 1, flags, Some(index), name, None),
        SymbolInfo::Event { flags, index, name } => ("tag", 4, flags, Some(index), name, None),
        SymbolInfo::Data {
            flags,
            name,
            symbol,
        } => ("data", 0, flags, None, Some(name), symbol),
        SymbolInfo::Section { .. } => return Ok(None),
    };
    let name = if let Some(name) = name {
        name.to_string()
    } else {
        imports
            .iter()
            .find(|item| item.kind == external_kind && Some(item.index) == index)
            .ok_or_else(|| {
                format!("linking symbol {symbol_index} has no matching import identity")
            })?
            .name
            .clone()
    };
    Ok(Some(WasmLinkingSymbolFact {
        symbol_index,
        name,
        kind,
        flags: flags.bits(),
        index,
        segment_index: data.map(|value| value.index),
        data_offset: data.map(|value| value.offset),
        size: data.map(|value| value.size),
    }))
}

fn parse_linking_symbols(
    linking: LinkingSectionReader<'_>,
    data_symbols: &mut Option<Vec<Option<LinkingDataSymbol>>>,
    linking_symbols: &mut Vec<WasmLinkingSymbolFact>,
    imports: &[WasmCanonicalImportTypeFact],
    got_symbols: &mut BTreeMap<String, GotSymbolBinding>,
    got_indices: &mut BTreeSet<u32>,
) -> Result<bool, String> {
    let mut symbol_table_seen = false;
    for subsection in linking {
        let subsection = subsection.map_err(|error| error.to_string())?;
        let Linking::SymbolTable(symbols) = subsection else {
            continue;
        };
        if symbol_table_seen || data_symbols.is_some() {
            return Err("duplicate WebAssembly linking symbol table".to_string());
        }
        symbol_table_seen = true;
        let mut indexed_data_symbols = Vec::with_capacity(symbols.count() as usize);
        for (symbol_index, symbol) in symbols.into_iter().enumerate() {
            let symbol = symbol.map_err(|error| error.to_string())?;
            if let Some(fact) = linking_symbol_fact(symbol, symbol_index as u32, imports)? {
                linking_symbols.push(fact);
            }
            let data_symbol = match symbol {
                SymbolInfo::Data {
                    flags,
                    name,
                    symbol,
                } => Some(LinkingDataSymbol {
                    name: name.to_string(),
                    flags,
                    defined: symbol.is_some(),
                }),
                SymbolInfo::Global { flags, index, name } => {
                    if let Some(name) = name
                        && let Some(got_symbol) = name.strip_prefix(SPLIT_RUNTIME_GOT_DATA_PREFIX)
                    {
                        if got_symbol.is_empty() {
                            return Err("empty split-runtime GOT data symbol".to_string());
                        }
                        if got_symbols.contains_key(got_symbol) {
                            return Err(format!(
                                "duplicate split-runtime GOT data symbol: {got_symbol}"
                            ));
                        }
                        insert_got_global(
                            got_symbol,
                            index,
                            flags,
                            !flags.contains(SymbolFlags::UNDEFINED),
                            got_symbols,
                            got_indices,
                        )?;
                    }
                    None
                }
                _ => None,
            };
            indexed_data_symbols.push(data_symbol);
        }
        *data_symbols = Some(indexed_data_symbols);
    }
    Ok(symbol_table_seen)
}

fn parse_code_got_relocations(
    relocation: RelocSectionReader<'_>,
    code_section_index: u32,
    code_payload: &[u8],
    data_symbols: &[Option<LinkingDataSymbol>],
    got_symbols: &mut BTreeMap<String, GotSymbolBinding>,
    got_indices: &mut BTreeSet<u32>,
) -> Result<(), String> {
    if relocation.section_index() != code_section_index {
        return Err(format!(
            "reloc.CODE targets section {}, expected code section {code_section_index}",
            relocation.section_index()
        ));
    }
    for entry in relocation.entries() {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.ty != RelocationType::GlobalIndexLeb {
            continue;
        }
        let symbol_index = usize::try_from(entry.index)
            .map_err(|_| "reloc.CODE symbol index exceeds host usize")?;
        let symbol_entry = data_symbols.get(symbol_index).ok_or_else(|| {
            format!(
                "reloc.CODE references missing linking symbol index {}",
                entry.index
            )
        })?;
        let Some(symbol) = symbol_entry.as_ref() else {
            continue;
        };
        let range = entry
            .relocation_range()
            .map_err(|error| error.to_string())?;
        let encoded = code_payload.get(range.clone()).ok_or_else(|| {
            format!(
                "reloc.CODE global-index range {}..{} exceeds code section size {}",
                range.start,
                range.end,
                code_payload.len()
            )
        })?;
        let original_offset =
            u64::try_from(range.start).map_err(|_| "reloc.CODE offset exceeds u64")?;
        let mut reader = BinaryReader::new(encoded, original_offset);
        let global_index = reader.read_var_u32().map_err(|error| error.to_string())?;
        if !reader.eof() {
            return Err(format!(
                "reloc.CODE global-index relocation at {} does not occupy its exact extent",
                entry.offset
            ));
        }
        insert_got_global(
            &symbol.name,
            global_index,
            symbol.flags,
            symbol.defined,
            got_symbols,
            got_indices,
        )?;
    }
    Ok(())
}

pub(crate) fn scan_wasm_link_facts_with_sections(
    bytes: &[u8],
    mut emit_section: Option<&mut SectionEmitter<'_>>,
) -> Result<WasmLinkFacts, String> {
    let mut function_import_count = 0u32;
    let mut global_import_count = 0u32;
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
    let mut main_module_init_function_index = None;
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
    let mut linking_section_seen = false;
    let mut linking_symbol_table_present = false;
    let mut linking_data_symbols = None;
    let mut linking_symbols = Vec::new();
    let mut function_names = BTreeMap::new();
    let mut code_relocations = Vec::new();
    let mut code_section_index = None;
    let mut code_section_range = None;
    let mut section_count = 0u32;
    let mut got_symbol_indices = BTreeMap::new();
    let mut got_global_indices = BTreeSet::new();
    let mut defined_global_shapes = BTreeMap::new();
    let mut canonical_import_types = Vec::new();
    let mut pending_exports = Vec::new();
    let mut global_types = Vec::new();
    let mut memory_types = Vec::new();
    let mut defined_memory_count = 0u32;
    let mut tag_types = Vec::new();
    let mut custom_section_names = Vec::new();
    let mut validator = Validator::new();
    let mut validator_allocations = FuncValidatorAllocations::default();
    let mut operator_reader_allocations = OperatorsReaderAllocations::default();
    let mut pending_code_section: Option<(u8, std::ops::Range<u64>, u32)> = None;
    let mut module_header_seen = false;

    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|error| error.to_string())?;
        let raw_section = payload.as_section();
        let current_section_index = if raw_section.is_some() {
            let index = section_count;
            section_count = section_count
                .checked_add(1)
                .ok_or("WebAssembly section count overflow")?;
            Some(index)
        } else {
            None
        };
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
                    let (kind, index, extern_type) = match import.ty {
                        TypeRef::Func(type_index) | TypeRef::FuncExact(type_index) => {
                            let exact = matches!(import.ty, TypeRef::FuncExact(_));
                            let function_index = function_import_count;
                            function_import_count = function_import_count
                                .checked_add(1)
                                .ok_or("function import count overflow")?;
                            function_import_type_indices.push(type_index);
                            if import.module == "env"
                                && import.name.strip_prefix("molt_call_indirect").is_some_and(
                                    |arity| {
                                        !arity.is_empty()
                                            && arity.bytes().all(|byte| byte.is_ascii_digit())
                                    },
                                )
                            {
                                dynamic_table_dispatch = true;
                                dynamic_dispatch_imports.insert(function_index);
                            }
                            (
                                0,
                                function_index,
                                canonical_function_type(type_index, exact, &function_types)?,
                            )
                        }
                        TypeRef::Table(table_type) => {
                            let table_index = u32::try_from(tables.len())
                                .map_err(|_| "table import index overflow")?;
                            let table = table_fact(table_type, true, tables.len())?;
                            let extern_type = canonical_table_type(&table);
                            tables.push(table);
                            (1, table_index, extern_type)
                        }
                        TypeRef::Memory(memory_type) => {
                            let memory_index = u32::try_from(memory_types.len())
                                .map_err(|_| "memory import index overflow")?;
                            let extern_type = canonical_memory_type(memory_type);
                            memory_types.push(extern_type.clone());
                            (2, memory_index, extern_type)
                        }
                        TypeRef::Global(global_type) => {
                            let global_index = global_import_count;
                            global_import_count = global_import_count
                                .checked_add(1)
                                .ok_or("global import count overflow")?;
                            let extern_type = canonical_global_type(global_type)?;
                            global_types.push(extern_type.clone());
                            (3, global_index, extern_type)
                        }
                        TypeRef::Tag(tag_type) => {
                            let tag_index = u32::try_from(tag_types.len())
                                .map_err(|_| "tag import index overflow")?;
                            let extern_type = canonical_tag_type(tag_type, &function_types)?;
                            tag_types.push(extern_type.clone());
                            (4, tag_index, extern_type)
                        }
                    };
                    canonical_import_types.push(WasmCanonicalImportTypeFact {
                        module: import.module.to_string(),
                        name: import.name.to_string(),
                        kind,
                        index,
                        extern_type,
                    });
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
            Payload::MemorySection(reader) => {
                for memory_type in reader {
                    let memory_type = memory_type.map_err(|error| error.to_string())?;
                    memory_types.push(canonical_memory_type(memory_type));
                    defined_memory_count = defined_memory_count
                        .checked_add(1)
                        .ok_or("defined memory count overflow")?;
                }
            }
            Payload::GlobalSection(reader) => {
                for global in reader {
                    let global = global.map_err(|error| error.to_string())?;
                    let defined_index = u32::try_from(defined_global_shapes.len())
                        .map_err(|_| "defined global count overflow")?;
                    let global_index = global_import_count
                        .checked_add(defined_index)
                        .ok_or("global index overflow")?;
                    let mut operators = global.init_expr.get_operators_reader();
                    let first = operators.read().map_err(|error| error.to_string())?;
                    let initial_address = match first {
                        Operator::I32Const { value } => {
                            if !matches!(
                                operators.read().map_err(|error| error.to_string())?,
                                Operator::End
                            ) || !operators.eof()
                            {
                                None
                            } else {
                                Some(value as u32)
                            }
                        }
                        _ => None,
                    };
                    defined_global_shapes
                        .insert(global_index, DefinedGlobalShape { initial_address });
                    global_types.push(canonical_global_type(global.ty)?);
                    collect_const_expr_ref_funcs(global.init_expr, &mut root_function_indices)?;
                }
            }
            Payload::TagSection(reader) => {
                for tag_type in reader {
                    let tag_type = tag_type.map_err(|error| error.to_string())?;
                    tag_types.push(canonical_tag_type(tag_type, &function_types)?);
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(|error| error.to_string())?;
                    if matches!(export.kind, ExternalKind::Func | ExternalKind::FuncExact) {
                        root_function_indices.push(export.index);
                        if export.name == "molt_init___main__" {
                            main_module_init_function_index = Some(export.index);
                        }
                    }
                    if export.kind == ExternalKind::Table {
                        exported_table_indices.push(export.index);
                    }
                    if export.name.starts_with("__molt_table_ref_") {
                        forbidden_callable_alias_exports.push(export.name.to_string());
                    }
                    let kind = match export.kind {
                        ExternalKind::Func => PendingExportKind::Function { exact: false },
                        ExternalKind::FuncExact => PendingExportKind::Function { exact: true },
                        ExternalKind::Global => PendingExportKind::Global,
                        ExternalKind::Memory => PendingExportKind::Memory,
                        ExternalKind::Table => PendingExportKind::Table,
                        ExternalKind::Tag => PendingExportKind::Tag,
                    };
                    pending_exports.push(PendingExport {
                        name: export.name.to_string(),
                        kind,
                        index: export.index,
                    });
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
                            (**emitter)(*id, section_bytes(bytes, range.clone())?)?;
                        }
                        pending_code_section = None;
                    }
                }
            }
            Payload::CodeSectionStart { count, range, .. } => {
                if pending_code_section.is_some() {
                    return Err("nested code section publication state".to_string());
                }
                if code_section_index.is_some() || code_section_range.is_some() {
                    return Err("duplicate code section".to_string());
                }
                code_section_index = current_section_index;
                code_section_range = Some(range.clone());
                pending_code_section = Some((10, range, count));
                if count == 0 {
                    let (id, range, _) = pending_code_section
                        .take()
                        .ok_or("missing empty code section publication state")?;
                    if let Some(emitter) = emit_section.as_mut() {
                        (**emitter)(id, section_bytes(bytes, range)?)?;
                    }
                }
            }
            Payload::CustomSection(reader) if reader.name() == CALLABLE_TABLE_SECTION_NAME => {
                custom_section_names.push(reader.name().to_string());
                if callable_table_attestation.is_some() {
                    return Err("duplicate molt.callable_table custom sections".to_string());
                }
                callable_table_attestation = Some(reader.data());
            }
            Payload::CustomSection(reader)
                if reader.name() == CALLABLE_TABLE_LAYOUT_SECTION_NAME =>
            {
                custom_section_names.push(reader.name().to_string());
                if callable_table_layout.is_some() {
                    return Err("duplicate molt.callable_table.layout custom sections".to_string());
                }
                callable_table_layout = Some(decode_callable_table_layout(reader.data())?);
            }
            Payload::CustomSection(reader) if reader.name() == "name" => {
                custom_section_names.push(reader.name().to_string());
                let KnownCustom::Name(names) = reader.as_known() else {
                    return Err("malformed WebAssembly name custom section".to_string());
                };
                for subsection in names {
                    if let wasmparser::Name::Function(names) =
                        subsection.map_err(|error| error.to_string())?
                    {
                        for name in names {
                            let name = name.map_err(|error| error.to_string())?;
                            if function_names
                                .insert(name.index, name.name.to_string())
                                .is_some()
                            {
                                return Err(format!(
                                    "duplicate function name index {}",
                                    name.index
                                ));
                            }
                        }
                    }
                }
            }
            Payload::CustomSection(reader) if reader.name() == "linking" => {
                custom_section_names.push(reader.name().to_string());
                if linking_section_seen {
                    return Err("duplicate WebAssembly linking custom section".to_string());
                }
                linking_section_seen = true;
                let KnownCustom::Linking(linking) = reader.as_known() else {
                    return Err("malformed WebAssembly linking custom section".to_string());
                };
                linking_symbol_table_present = parse_linking_symbols(
                    linking,
                    &mut linking_data_symbols,
                    &mut linking_symbols,
                    &canonical_import_types,
                    &mut got_symbol_indices,
                    &mut got_global_indices,
                )?;
            }
            Payload::CustomSection(reader) if reader.name() == "reloc.CODE" => {
                custom_section_names.push(reader.name().to_string());
                let KnownCustom::Reloc(relocation) = reader.as_known() else {
                    return Err("malformed WebAssembly reloc.CODE custom section".to_string());
                };
                if !code_relocations.is_empty() {
                    return Err("duplicate WebAssembly reloc.CODE custom section".to_string());
                }
                code_relocations.push(relocation);
            }
            Payload::CustomSection(reader) => {
                custom_section_names.push(reader.name().to_string());
            }
            _ => {}
        }
        if !replaced_custom_section
            && !deferred_code_section
            && let Some((id, range)) = raw_section
            && let Some(emitter) = emit_section.as_mut()
        {
            (**emitter)(id, section_bytes(bytes, range)?)?;
        }
    }

    if pending_code_section.is_some() {
        return Err("code section ended before all declared bodies were decoded".to_string());
    }
    if !module_header_seen {
        return Err("missing top-level WebAssembly module header".to_string());
    }
    if let Some(relocation) = code_relocations.pop() {
        let code_section_index = code_section_index.ok_or("reloc.CODE has no code section")?;
        let code_section_range = code_section_range.ok_or("reloc.CODE has no code section")?;
        let code_payload = section_bytes(bytes, code_section_range)?;
        let data_symbols = linking_data_symbols
            .as_deref()
            .ok_or("reloc.CODE has no linking symbol-table authority")?;
        parse_code_got_relocations(
            relocation,
            code_section_index,
            code_payload,
            data_symbols,
            &mut got_symbol_indices,
            &mut got_global_indices,
        )?;
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
    canonical_import_types.sort();
    let mut canonical_export_types = pending_exports
        .into_iter()
        .map(|export| {
            let extern_type = match export.kind {
                PendingExportKind::Function { exact } => {
                    let type_index = *function_type_indices
                        .get(export.index as usize)
                        .ok_or_else(|| {
                            format!(
                                "function export {} has out-of-range index {}",
                                export.name, export.index
                            )
                        })?;
                    canonical_function_type(type_index, exact, &function_types)?
                }
                PendingExportKind::Global => global_types
                    .get(export.index as usize)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "global export {} has out-of-range index {}",
                            export.name, export.index
                        )
                    })?,
                PendingExportKind::Memory => memory_types
                    .get(export.index as usize)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "memory export {} has out-of-range index {}",
                            export.name, export.index
                        )
                    })?,
                PendingExportKind::Table => tables
                    .get(export.index as usize)
                    .map(canonical_table_type)
                    .ok_or_else(|| {
                        format!(
                            "table export {} has out-of-range index {}",
                            export.name, export.index
                        )
                    })?,
                PendingExportKind::Tag => tag_types
                    .get(export.index as usize)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "tag export {} has out-of-range index {}",
                            export.name, export.index
                        )
                    })?,
            };
            Ok(WasmCanonicalExportTypeFact {
                name: export.name,
                kind: export.kind.external_kind(),
                index: export.index,
                extern_type,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    canonical_export_types.sort();
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
    let reachable_function_indices = reachable_functions
        .iter()
        .enumerate()
        .filter_map(|(index, reachable)| reachable.then_some(index as u32))
        .collect::<Vec<_>>();
    let mut referenced_functions = vec![false; function_type_indices.len()];
    for function_index in root_function_indices
        .iter()
        .chain(&element_function_indices)
        .chain(&declared_function_indices)
        .copied()
        .chain(
            function_references
                .iter()
                .flat_map(|row| row.direct_calls.iter().chain(&row.ref_funcs).copied()),
        )
    {
        let referenced = referenced_functions
            .get_mut(function_index as usize)
            .ok_or_else(|| format!("function reference index {function_index} is out of range"))?;
        *referenced = true;
    }
    let referenced_function_indices = referenced_functions
        .iter()
        .enumerate()
        .filter_map(|(index, referenced)| referenced.then_some(index as u32))
        .collect::<Vec<_>>();
    let main_module_init_direct_calls = main_module_init_function_index
        .and_then(|function_index| {
            reference_row_by_function
                .get(function_index as usize)
                .and_then(|row| *row)
        })
        .map(|row| function_references[row].direct_calls.clone())
        .unwrap_or_default();
    // Record evidence for every relocation. Binding and shape policy belongs to
    // the consumer selecting CPython-ABI globals; unrelated weak/local/undefined
    // PIC symbols are valid linker inputs and must not fail the module scan.
    for (symbol, binding) in &got_symbol_indices {
        if binding.global_index as usize >= global_types.len() {
            return Err(format!(
                "split-runtime GOT data symbol {symbol} references missing global index {}",
                binding.global_index
            ));
        }
    }
    let split_runtime_got_data_globals = got_symbol_indices
        .into_iter()
        .map(|(symbol, binding)| WasmGotDataGlobalFact {
            symbol,
            global_index: binding.global_index,
            initial_address: defined_global_shapes
                .get(&binding.global_index)
                .and_then(|shape| shape.initial_address),
            flags: binding.flags.bits(),
            defined: binding.defined,
        })
        .collect();
    Ok(WasmLinkFacts {
        schema_version: WASM_LINK_FACTS_SCHEMA_VERSION,
        function_import_count,
        defined_function_count: declared_function_count,
        code_body_count: defined_function_index,
        operator_count,
        reachable_function_indices,
        referenced_function_indices,
        main_module_init_direct_calls,
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
        defined_memory_count,
        custom_section_names,
        linking_symbol_table_present,
        linking_symbols,
        function_names: function_names.into_iter().collect(),
        split_runtime_got_data_globals,
        canonical_import_types,
        canonical_export_types,
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
