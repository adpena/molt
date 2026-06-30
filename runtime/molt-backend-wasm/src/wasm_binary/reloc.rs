use crate::wasm_abi::{CALL_INDIRECT_IMPORTS, wasm_runtime_export_name};
use crate::wasm_data::{DataRelocSite, DataSegmentInfo};
use std::borrow::Cow;
use std::collections::BTreeMap;

use wasm_encoder::{CustomSection, DataSymbolDefinition, Encode, LinkingSection, SymbolTable};
use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

#[derive(Clone, Copy)]
enum PendingReloc {
    Function { offset: u32, func_index: u32 },
    Type { offset: u32, type_index: u32 },
    DataAddr { offset: u32, segment_index: u32 },
}

#[derive(Clone, Copy)]
struct RelocEntry {
    ty: u8,
    offset: u32,
    index: u32,
    addend: i32,
}

fn is_manifest_call_indirect_import_name(name: &str) -> bool {
    CALL_INDIRECT_IMPORTS
        .iter()
        .any(|spec| spec.import_name == name)
}

fn encode_reloc_section(
    name: &'static str,
    section_index: u32,
    entries: &[RelocEntry],
) -> CustomSection<'static> {
    let mut data = Vec::new();
    section_index.encode(&mut data);
    (entries.len() as u32).encode(&mut data);
    for entry in entries {
        data.push(entry.ty);
        entry.offset.encode(&mut data);
        entry.index.encode(&mut data);
        if matches!(entry.ty, 4 | 5) {
            entry.addend.encode(&mut data);
        }
    }
    CustomSection {
        name: name.into(),
        data: Cow::Owned(data),
    }
}

fn append_custom_section(bytes: &mut Vec<u8>, section: &impl Encode) {
    bytes.push(0);
    section.encode(bytes);
}

pub(crate) fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<String> = Vec::new();
    let mut func_exports: BTreeMap<u32, String> = BTreeMap::new();
    let mut func_import_count = 0u32;
    let mut defined_func_count = 0u32;
    let mut table_import_count = 0u32;
    let mut table_defined_count = 0u32;
    let mut code_section_start = None;
    let mut code_section_index = None;
    let mut data_section_index = None;
    let mut element_section_index = None;
    let mut func_body_starts: Vec<usize> = Vec::new();
    let mut pending_code: Vec<PendingReloc> = Vec::new();
    let mut pending_data: Vec<PendingReloc> = Vec::new();
    let mut pending_elem: Vec<PendingReloc> = Vec::new();
    let mut section_index = 0u32;

    let mut parse_failed = false;
    for payload in Parser::new(0).parse_all(&bytes) {
        let payload = match payload {
            Ok(payload) => payload,
            Err(_) => {
                parse_failed = true;
                break;
            }
        };
        match payload {
            Payload::TypeSection(_) => {
                section_index += 1;
            }
            Payload::ImportSection(reader) => {
                section_index += 1;
                for import in reader.into_imports().flatten() {
                    match import.ty {
                        TypeRef::Func(_) => {
                            func_imports.push(import.name.to_string());
                            func_import_count += 1;
                        }
                        TypeRef::Table(_) => {
                            table_import_count += 1;
                        }
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                defined_func_count = reader.count();
                section_index += 1;
            }
            Payload::TableSection(reader) => {
                table_defined_count = reader.count();
                section_index += 1;
            }
            Payload::MemorySection(_) => {
                section_index += 1;
            }
            Payload::GlobalSection(_) => {
                section_index += 1;
            }
            Payload::ExportSection(reader) => {
                for export in reader.into_iter().flatten() {
                    if export.kind == ExternalKind::Func {
                        func_exports.insert(export.index, export.name.to_string());
                    }
                }
                section_index += 1;
            }
            Payload::StartSection { .. } => {
                section_index += 1;
            }
            Payload::ElementSection(reader) => {
                let element_section_start = reader.range().start;
                element_section_index = Some(section_index);
                section_index += 1;
                for element in reader.into_iter().flatten() {
                    if let ElementItems::Functions(funcs) = element.items {
                        for func in funcs.into_iter_with_offsets().flatten() {
                            let (pos, func_index) = func;
                            let offset = (pos.saturating_sub(element_section_start)) as u32;
                            pending_elem.push(PendingReloc::Function { offset, func_index });
                        }
                    }
                }
            }
            Payload::CodeSectionStart { range, .. } => {
                code_section_start = Some(range.start);
                code_section_index = Some(section_index);
                section_index += 1;
            }
            Payload::CodeSectionEntry(body) => {
                func_body_starts.push(body.range().start);
                if let Ok(mut ops) = body.get_operators_reader() {
                    while let Ok((op, op_offset)) = ops.read_with_offset() {
                        let start = match code_section_start {
                            Some(start) => start,
                            None => break,
                        };
                        match op {
                            Operator::Call { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            Operator::CallIndirect { type_index, .. } => {
                                let type_offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Type {
                                    offset: type_offset,
                                    type_index,
                                });
                            }
                            Operator::RefFunc { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Payload::DataSection(reader) => {
                let data_section_start = reader.range().start;
                data_section_index = Some(section_index);
                section_index += 1;
                for (segment_index, data) in reader.into_iter().enumerate() {
                    if let Ok(data) = data
                        && let DataKind::Active { offset_expr, .. } = data.kind
                    {
                        let mut ops = offset_expr.get_operators_reader();
                        if let Ok((Operator::I32Const { .. }, op_offset)) = ops.read_with_offset() {
                            let offset = (op_offset + 1).saturating_sub(data_section_start) as u32;
                            pending_data.push(PendingReloc::DataAddr {
                                offset,
                                segment_index: segment_index as u32,
                            });
                        }
                    }
                }
            }
            Payload::DataCountSection { .. } => {
                section_index += 1;
            }
            _ => {}
        }
    }
    if parse_failed {
        return bytes;
    }

    let code_section_start = match code_section_start {
        Some(start) => start,
        None => return bytes,
    };
    let code_section_index = match code_section_index {
        Some(index) => index,
        None => return bytes,
    };
    let data_section_index = data_section_index;

    for site in data_relocs {
        let def_index = site.defined_func_index as usize;
        if let Some(body_start) = func_body_starts.get(def_index) {
            let offset = (body_start.saturating_sub(code_section_start) as u32)
                .saturating_add(site.offset_in_func);
            pending_code.push(PendingReloc::DataAddr {
                offset,
                segment_index: site.segment_index,
            });
        }
    }

    let total_funcs = func_import_count + defined_func_count;
    let mut func_symbol_map = vec![0u32; total_funcs as usize];
    let mut data_symbol_map = vec![0u32; data_segments.len()];
    let mut symbol_index = 0u32;

    let mut sym_tab = SymbolTable::new();
    let mut import_names: Vec<String> = Vec::new();
    for (idx, name) in func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = wasm_runtime_export_name(name)
            .unwrap_or_else(|| panic!("missing generated runtime export for import {name}"))
            .to_string();
        import_names.push(symbol_name);
        let name_ref = import_names.last().unwrap();
        sym_tab.function(flags, idx as u32, Some(name_ref));
        func_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }
    let mut func_names: Vec<String> = Vec::new();
    for def_idx in 0..defined_func_count {
        let func_index = func_import_count + def_idx;
        let export_name = func_exports.get(&func_index).cloned();
        // Keep linker symbol names module-scoped so linked output/runtime objects
        // cannot accidentally alias local function symbols with identical names.
        // Preserve explicit call_indirect export symbols because wasm_link.py
        // resolves/aliases those by name for runtime ABI wiring.
        let name = match export_name.as_deref() {
            Some("molt_host_init") | Some("molt_main") | Some("molt_table_init") => {
                export_name.clone().unwrap_or_default()
            }
            Some(exported) if is_manifest_call_indirect_import_name(exported) => {
                exported.to_string()
            }
            Some(_) => format!("__molt_output_export_{func_index}"),
            None => format!("__molt_output_fn_{func_index}"),
        };
        func_names.push(name);
        let name_ref = func_names.last().unwrap();
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        sym_tab.function(flags, func_index, Some(name_ref));
        func_symbol_map[func_index as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        sym_tab.table(flags, table_idx, None);
        symbol_index += 1;
    }
    let mut table_names: Vec<String> = Vec::new();
    for table_idx in 0..table_defined_count {
        let index = table_import_count + table_idx;
        let name = format!("__molt_output_table_{index}");
        table_names.push(name);
        let name_ref = table_names.last().unwrap();
        sym_tab.table(0, index, Some(name_ref));
        symbol_index += 1;
    }

    let mut data_names: Vec<String> = Vec::new();
    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_output_data_{idx}");
        data_names.push(name);
        let name_ref = data_names.last().unwrap();
        sym_tab.data(
            0,
            name_ref,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }

    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();
    for reloc in pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = func_symbol_map.get(func_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 0,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::Type { offset, type_index } => {
                code_entries.push(RelocEntry {
                    ty: 6,
                    offset,
                    index: type_index,
                    addend: 0,
                });
            }
            PendingReloc::DataAddr {
                offset,
                segment_index,
            } => {
                if let Some(index) = data_symbol_map.get(segment_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 4,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
        }
    }

    for reloc in pending_data {
        if let PendingReloc::DataAddr {
            offset,
            segment_index,
        } = reloc
            && let Some(index) = data_symbol_map.get(segment_index as usize)
        {
            data_entries.push(RelocEntry {
                ty: 4,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    for reloc in pending_elem {
        if let PendingReloc::Function { offset, func_index } = reloc
            && let Some(index) = func_symbol_map.get(func_index as usize)
        {
            elem_entries.push(RelocEntry {
                ty: 0,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    code_entries.sort_by_key(|entry| entry.offset);
    data_entries.sort_by_key(|entry| entry.offset);
    elem_entries.sort_by_key(|entry| entry.offset);

    let mut linking = LinkingSection::new();
    linking.symbol_table(&sym_tab);
    append_custom_section(&mut bytes, &linking);
    if !code_entries.is_empty() {
        let reloc_code = encode_reloc_section("reloc.CODE", code_section_index, &code_entries);
        append_custom_section(&mut bytes, &reloc_code);
    }
    if !data_entries.is_empty()
        && let Some(index) = data_section_index
    {
        let reloc_data = encode_reloc_section("reloc.DATA", index, &data_entries);
        append_custom_section(&mut bytes, &reloc_data);
    }
    if !elem_entries.is_empty()
        && let Some(index) = element_section_index
    {
        let reloc_elem = encode_reloc_section("reloc.ELEM", index, &elem_entries);
        append_custom_section(&mut bytes, &reloc_elem);
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::{add_reloc_sections, is_manifest_call_indirect_import_name};
    use crate::wasm_abi::TypeSectionExt;
    use crate::wasm_binary::strip_unused_imports;
    use crate::wasm_data::WasmDataSegments;
    use std::collections::BTreeSet;
    use wasm_encoder::{
        CodeSection, EntityType, Function, FunctionSection, ImportSection, Instruction, Module,
        TypeSection,
    };
    use wasmparser::{Parser, Payload};

    fn read_varuint(data: &[u8], mut offset: usize) -> (u32, usize) {
        let mut result = 0u32;
        let mut shift = 0u32;
        loop {
            let byte = data[offset];
            offset += 1;
            result |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return (result, offset);
            }
            shift += 7;
        }
    }

    fn code_body_ranges(wasm: &[u8]) -> (usize, Vec<std::ops::Range<usize>>) {
        let mut code_start = None;
        let mut ranges = Vec::new();
        for payload in Parser::new(0).parse_all(wasm) {
            match payload.expect("valid wasm payload") {
                Payload::CodeSectionStart { range, .. } => {
                    code_start = Some(range.start);
                }
                Payload::CodeSectionEntry(body) => {
                    ranges.push(body.range());
                }
                _ => {}
            }
        }
        (code_start.expect("code section start"), ranges)
    }

    fn reloc_code_memory_addr_offsets(wasm: &[u8]) -> Vec<u32> {
        let mut offsets = Vec::new();
        for payload in Parser::new(0).parse_all(wasm) {
            let Payload::CustomSection(section) = payload.expect("valid wasm payload") else {
                continue;
            };
            if section.name() != "reloc.CODE" {
                continue;
            }
            let data = section.data();
            let (_, cursor) = read_varuint(data, 0);
            let (count, mut cursor) = read_varuint(data, cursor);
            for _ in 0..count {
                let ty = data[cursor];
                cursor += 1;
                let (offset, next) = read_varuint(data, cursor);
                cursor = next;
                let (_, next) = read_varuint(data, cursor);
                cursor = next;
                if matches!(ty, 4 | 5) {
                    let (_, next) = read_varuint(data, cursor);
                    cursor = next;
                }
                if ty == 4 {
                    offsets.push(offset);
                }
            }
        }
        offsets
    }

    #[test]
    fn call_indirect_symbol_preservation_uses_manifest_membership() {
        assert!(is_manifest_call_indirect_import_name("molt_call_indirect0"));
        assert!(is_manifest_call_indirect_import_name(
            "molt_call_indirect13"
        ));
        assert!(!is_manifest_call_indirect_import_name(
            "molt_call_indirect99"
        ));
        assert!(!is_manifest_call_indirect_import_name("molt_call_indirect"));
    }

    #[test]
    fn data_reloc_sites_follow_defined_body_ordinal_after_import_strip() {
        let mut types = TypeSection::new();
        types.function([], []);

        let mut imports = ImportSection::new();
        imports.import("molt_runtime", "types_bootstrap", EntityType::Function(0));
        imports.import("molt_runtime", "abc_bootstrap", EntityType::Function(0));

        let mut funcs = FunctionSection::new();
        funcs.function(0);
        funcs.function(0);

        let mut codes = CodeSection::new();
        let mut first = Function::new([]);
        first.instruction(&Instruction::End);
        codes.function(&first);

        let mut data_segments = WasmDataSegments::new(64 * 1024 * 1024);
        let data_ref = data_segments.add_segment(true, b"molt");
        let mut second = Function::new([]);
        data_segments.emit_ptr_i32(true, 1, &mut second, data_ref);
        second.instruction(&Instruction::Drop);
        second.instruction(&Instruction::End);
        codes.function(&second);

        let mut module = Module::new();
        module.section(&types);
        module.section(&imports);
        module.section(&funcs);
        module.section(&codes);
        module.section(data_segments.section());

        let mut unused = BTreeSet::new();
        unused.insert("types_bootstrap".to_string());
        let stripped = strip_unused_imports(module.finish(), &unused);
        let relocated =
            add_reloc_sections(stripped, data_segments.segments(), data_segments.relocs());

        let (code_start, body_ranges) = code_body_ranges(&relocated);
        assert_eq!(body_ranges.len(), 2);
        let second_body = &body_ranges[1];
        let second_start = (second_body.start - code_start) as u32;
        let second_end = (second_body.end - code_start) as u32;

        let offsets = reloc_code_memory_addr_offsets(&relocated);
        assert_eq!(offsets.len(), 1);
        assert!(
            (second_start..second_end).contains(&offsets[0]),
            "data relocation offset {} must target second defined body range {}..{}",
            offsets[0],
            second_start,
            second_end,
        );
    }
}
