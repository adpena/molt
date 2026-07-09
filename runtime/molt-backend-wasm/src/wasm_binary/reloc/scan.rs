use std::collections::BTreeMap;

use crate::wasm_data::DataRelocSite;

use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

use super::types::{FunctionImport, PendingReloc};

pub(super) struct RelocScan {
    pub(super) func_imports: Vec<FunctionImport>,
    pub(super) func_exports: BTreeMap<u32, String>,
    pub(super) func_import_count: u32,
    pub(super) defined_func_count: u32,
    pub(super) table_import_count: u32,
    pub(super) table_defined_count: u32,
    pub(super) code_section_start: Option<usize>,
    pub(super) code_section_index: Option<u32>,
    pub(super) data_section_index: Option<u32>,
    pub(super) element_section_index: Option<u32>,
    pub(super) func_body_starts: Vec<usize>,
    pub(super) pending_code: Vec<PendingReloc>,
    pub(super) pending_data: Vec<PendingReloc>,
    pub(super) pending_elem: Vec<PendingReloc>,
}

impl RelocScan {
    pub(super) fn collect(bytes: &[u8]) -> Option<Self> {
        let mut scan = Self {
            func_imports: Vec::new(),
            func_exports: BTreeMap::new(),
            func_import_count: 0,
            defined_func_count: 0,
            table_import_count: 0,
            table_defined_count: 0,
            code_section_start: None,
            code_section_index: None,
            data_section_index: None,
            element_section_index: None,
            func_body_starts: Vec::new(),
            pending_code: Vec::new(),
            pending_data: Vec::new(),
            pending_elem: Vec::new(),
        };
        let mut section_index = 0u32;

        for payload in Parser::new(0).parse_all(bytes) {
            let payload = match payload {
                Ok(payload) => payload,
                Err(_) => return None,
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
                                scan.func_imports.push(FunctionImport {
                                    module: import.module.to_string(),
                                    name: import.name.to_string(),
                                });
                                scan.func_import_count += 1;
                            }
                            TypeRef::Table(_) => {
                                scan.table_import_count += 1;
                            }
                            _ => {}
                        }
                    }
                }
                Payload::FunctionSection(reader) => {
                    scan.defined_func_count = reader.count();
                    section_index += 1;
                }
                Payload::TableSection(reader) => {
                    scan.table_defined_count = reader.count();
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
                            scan.func_exports
                                .insert(export.index, export.name.to_string());
                        }
                    }
                    section_index += 1;
                }
                Payload::StartSection { .. } => {
                    section_index += 1;
                }
                Payload::ElementSection(reader) => {
                    let element_section_start = reader.range().start;
                    scan.element_section_index = Some(section_index);
                    section_index += 1;
                    for element in reader.into_iter().flatten() {
                        if let ElementItems::Functions(funcs) = element.items {
                            for func in funcs.into_iter_with_offsets().flatten() {
                                let (pos, func_index) = func;
                                let offset = (pos.saturating_sub(element_section_start)) as u32;
                                scan.pending_elem
                                    .push(PendingReloc::Function { offset, func_index });
                            }
                        }
                    }
                }
                Payload::CodeSectionStart { range, .. } => {
                    scan.code_section_start = Some(range.start);
                    scan.code_section_index = Some(section_index);
                    section_index += 1;
                }
                Payload::CodeSectionEntry(body) => {
                    scan.func_body_starts.push(body.range().start);
                    if let Ok(mut ops) = body.get_operators_reader() {
                        while let Ok((op, op_offset)) = ops.read_with_offset() {
                            let start = match scan.code_section_start {
                                Some(start) => start,
                                None => break,
                            };
                            match op {
                                Operator::Call { function_index } => {
                                    let offset = (op_offset + 1).saturating_sub(start) as u32;
                                    scan.pending_code.push(PendingReloc::Function {
                                        offset,
                                        func_index: function_index,
                                    });
                                }
                                Operator::CallIndirect { type_index, .. } => {
                                    let type_offset = (op_offset + 1).saturating_sub(start) as u32;
                                    scan.pending_code.push(PendingReloc::Type {
                                        offset: type_offset,
                                        type_index,
                                    });
                                }
                                Operator::RefFunc { function_index } => {
                                    let offset = (op_offset + 1).saturating_sub(start) as u32;
                                    scan.pending_code.push(PendingReloc::Function {
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
                    scan.data_section_index = Some(section_index);
                    section_index += 1;
                    for (segment_index, data) in reader.into_iter().enumerate() {
                        if let Ok(data) = data
                            && let DataKind::Active { offset_expr, .. } = data.kind
                        {
                            let mut ops = offset_expr.get_operators_reader();
                            if let Ok((Operator::I32Const { .. }, op_offset)) =
                                ops.read_with_offset()
                            {
                                let offset =
                                    (op_offset + 1).saturating_sub(data_section_start) as u32;
                                scan.pending_data.push(PendingReloc::DataAddr {
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

        Some(scan)
    }

    pub(super) fn record_data_reloc_sites(
        &mut self,
        data_relocs: &[DataRelocSite],
        code_section_start: usize,
    ) {
        for site in data_relocs {
            let def_index = site.defined_func_index as usize;
            if let Some(body_start) = self.func_body_starts.get(def_index) {
                let offset = (body_start.saturating_sub(code_section_start) as u32)
                    .saturating_add(site.offset_in_func);
                self.pending_code.push(PendingReloc::DataAddr {
                    offset,
                    segment_index: site.segment_index,
                });
            }
        }
    }
}
