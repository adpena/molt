use crate::wasm_data::{DataRelocSite, DataSegmentInfo};
use std::collections::BTreeMap;

use wasm_encoder::LinkingSection;
use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

mod sections;
mod symbols;

use sections::{RelocEntry, append_custom_section, encode_reloc_section};
use symbols::{FunctionImport, build_reloc_symbol_table};

#[derive(Clone, Copy)]
enum PendingReloc {
    Function { offset: u32, func_index: u32 },
    Type { offset: u32, type_index: u32 },
    DataAddr { offset: u32, segment_index: u32 },
}

pub(crate) fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<FunctionImport> = Vec::new();
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
                            func_imports.push(FunctionImport {
                                module: import.module.to_string(),
                                name: import.name.to_string(),
                            });
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

    let symbol_table = build_reloc_symbol_table(
        &func_imports,
        &func_exports,
        func_import_count,
        defined_func_count,
        table_import_count,
        table_defined_count,
        data_segments,
    );

    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();
    for reloc in pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = symbol_table.function_symbols.get(func_index as usize) {
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
                if let Some(index) = symbol_table.data_symbols.get(segment_index as usize) {
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
            && let Some(index) = symbol_table.data_symbols.get(segment_index as usize)
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
            && let Some(index) = symbol_table.function_symbols.get(func_index as usize)
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
    linking.symbol_table(&symbol_table.table);
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
mod tests;
