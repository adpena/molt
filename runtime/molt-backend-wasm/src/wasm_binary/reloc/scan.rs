use std::collections::BTreeMap;

use crate::wasm_data::DataRelocSite;
use crate::wasm_table::TableRelocSite;

use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

use super::types::{FunctionImport, PendingReloc};

fn checked_section_offset(position: usize, section_start: usize, context: &str) -> u32 {
    let relative = position.checked_sub(section_start).unwrap_or_else(|| {
        panic!("{context} position {position} precedes section start {section_start}")
    });
    u32::try_from(relative).unwrap_or_else(|_| panic!("{context} offset exceeds u32: {relative}"))
}

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
    pub(super) func_body_ranges: Vec<std::ops::Range<usize>>,
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
            func_body_ranges: Vec::new(),
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
            let payload_section_index = payload.as_section().map(|_| {
                let current = section_index;
                section_index = section_index
                    .checked_add(1)
                    .expect("WASM object section count exceeds u32");
                current
            });
            match payload {
                Payload::TypeSection(_) => {}
                Payload::ImportSection(reader) => {
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
                }
                Payload::TableSection(reader) => {
                    scan.table_defined_count = reader.count();
                }
                Payload::MemorySection(_) => {}
                Payload::GlobalSection(_) => {}
                Payload::ExportSection(reader) => {
                    for export in reader.into_iter().flatten() {
                        if export.kind == ExternalKind::Func {
                            scan.func_exports
                                .insert(export.index, export.name.to_string());
                        }
                    }
                }
                Payload::StartSection { .. } => {}
                Payload::ElementSection(reader) => {
                    let element_section_start = reader.range().start;
                    scan.element_section_index = payload_section_index;
                    for element in reader.into_iter().flatten() {
                        if let ElementItems::Functions(funcs) = element.items {
                            for func in funcs.into_iter_with_offsets().flatten() {
                                let (pos, func_index) = func;
                                let offset = checked_section_offset(
                                    pos,
                                    element_section_start,
                                    "element relocation",
                                );
                                scan.pending_elem
                                    .push(PendingReloc::Function { offset, func_index });
                            }
                        }
                    }
                }
                Payload::CodeSectionStart { range, .. } => {
                    scan.code_section_start = Some(range.start);
                    scan.code_section_index = payload_section_index;
                }
                Payload::CodeSectionEntry(body) => {
                    scan.func_body_ranges.push(body.range());
                    if let Ok(mut ops) = body.get_operators_reader() {
                        while let Ok((op, op_offset)) = ops.read_with_offset() {
                            let start = match scan.code_section_start {
                                Some(start) => start,
                                None => break,
                            };
                            match op {
                                Operator::Call { function_index } => {
                                    let offset = checked_section_offset(
                                        op_offset.checked_add(1).expect("call offset overflow"),
                                        start,
                                        "call relocation",
                                    );
                                    scan.pending_code.push(PendingReloc::Function {
                                        offset,
                                        func_index: function_index,
                                    });
                                }
                                Operator::CallIndirect { type_index, .. } => {
                                    let type_offset = checked_section_offset(
                                        op_offset
                                            .checked_add(1)
                                            .expect("call_indirect offset overflow"),
                                        start,
                                        "call_indirect type relocation",
                                    );
                                    scan.pending_code.push(PendingReloc::Type {
                                        offset: type_offset,
                                        type_index,
                                    });
                                }
                                Operator::RefFunc { function_index } => {
                                    let offset = checked_section_offset(
                                        op_offset.checked_add(1).expect("ref.func offset overflow"),
                                        start,
                                        "ref.func relocation",
                                    );
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
                    scan.data_section_index = payload_section_index;
                    for (segment_index, data) in reader.into_iter().enumerate() {
                        if let Ok(data) = data
                            && let DataKind::Active { offset_expr, .. } = data.kind
                        {
                            let mut ops = offset_expr.get_operators_reader();
                            if let Ok((Operator::I32Const { .. }, op_offset)) =
                                ops.read_with_offset()
                            {
                                let offset = checked_section_offset(
                                    op_offset
                                        .checked_add(1)
                                        .expect("data relocation offset overflow"),
                                    data_section_start,
                                    "data relocation",
                                );
                                scan.pending_data.push(PendingReloc::DataAddr {
                                    offset,
                                    segment_index: segment_index as u32,
                                });
                            }
                        }
                    }
                }
                Payload::DataCountSection { .. } => {}
                _ => {}
            }
        }

        Some(scan)
    }

    pub(super) fn record_data_reloc_sites(
        &mut self,
        bytes: &[u8],
        data_relocs: &[DataRelocSite],
        code_section_start: usize,
    ) {
        for site in data_relocs {
            let def_index = site.defined_func_index as usize;
            if let Some(body) = self.func_body_ranges.get(def_index) {
                let body_offset =
                    checked_section_offset(body.start, code_section_start, "data relocation owner");
                let offset = body_offset
                    .checked_add(site.offset_in_func)
                    .expect("data relocation site offset overflow");
                validate_padded_i32_operand(bytes, body, offset, code_section_start, "data")
                    .unwrap_or_else(|error| panic!("{error}"));
                self.pending_code.push(PendingReloc::DataAddr {
                    offset,
                    segment_index: site.segment_index,
                });
            }
        }
    }

    pub(super) fn record_table_reloc_sites(
        &mut self,
        bytes: &[u8],
        table_relocs: &[TableRelocSite],
        code_section_start: usize,
    ) {
        for site in table_relocs {
            let def_index = site.defined_func_index as usize;
            let body = self.func_body_ranges.get(def_index).unwrap_or_else(|| {
                panic!(
                    "callable-table relocation owner body {} is missing after import stripping",
                    site.defined_func_index
                )
            });
            let body_offset = checked_section_offset(
                body.start,
                code_section_start,
                "callable-table relocation owner",
            );
            let offset = body_offset
                .checked_add(site.offset_in_func)
                .expect("callable-table relocation site offset overflow");
            validate_padded_i32_operand(bytes, body, offset, code_section_start, "callable-table")
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}; owner={}, target={:?}, role={:?}",
                        site.defined_func_index, site.target, site.role
                    )
                });
            self.pending_code.push(PendingReloc::TableIndex {
                offset,
                target: site.target.clone(),
                role: site.role,
            });
        }
    }
}

fn validate_padded_i32_operand(
    bytes: &[u8],
    body: &std::ops::Range<usize>,
    section_offset: u32,
    code_section_start: usize,
    label: &str,
) -> Result<(), String> {
    let operand = code_section_start
        .checked_add(section_offset as usize)
        .ok_or_else(|| format!("{label} relocation absolute offset overflow"))?;
    let opcode = operand
        .checked_sub(1)
        .ok_or_else(|| format!("{label} relocation has no preceding opcode"))?;
    let encoded_end = operand
        .checked_add(5)
        .ok_or_else(|| format!("{label} relocation operand boundary overflow"))?;
    if opcode < body.start || encoded_end > body.end {
        return Err(format!(
            "{label} relocation operand {operand}..{encoded_end} escapes function body {}..{}",
            body.start, body.end
        ));
    }
    if bytes.get(opcode) != Some(&0x41) {
        return Err(format!(
            "{label} relocation at {operand} is not the operand of i32.const"
        ));
    }
    let encoded = bytes
        .get(operand..encoded_end)
        .ok_or_else(|| format!("{label} relocation operand exceeds module bytes"))?;
    if encoded[..4].iter().any(|byte| byte & 0x80 == 0) || encoded[4] & 0x80 != 0 {
        return Err(format!(
            "{label} relocation at {operand} does not target a padded five-byte SLEB"
        ));
    }
    Ok(())
}
