use super::scan::RelocScan;
use super::symbols::SymbolMaps;
use super::types::{PendingReloc, RelocEntry};

pub(super) struct RelocEntries {
    pub(super) code: Vec<RelocEntry>,
    pub(super) data: Vec<RelocEntry>,
    pub(super) elem: Vec<RelocEntry>,
}

pub(super) fn resolve_reloc_entries(scan: &RelocScan, maps: &SymbolMaps) -> RelocEntries {
    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();

    for reloc in &scan.pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = maps.func_symbol_map.get(*func_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 0,
                        offset: *offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::Type { offset, type_index } => {
                code_entries.push(RelocEntry {
                    ty: 6,
                    offset: *offset,
                    index: *type_index,
                    addend: 0,
                });
            }
            PendingReloc::DataAddr {
                offset,
                segment_index,
            } => {
                if let Some(index) = maps.data_symbol_map.get(*segment_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 4,
                        offset: *offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::TableIndex {
                offset,
                target,
                role,
            } => {
                let index = maps.function_symbol(target).unwrap_or_else(|| {
                    panic!(
                        "callable-table relocation target missing after import stripping: {target:?} ({role:?})"
                    )
                });
                code_entries.push(RelocEntry {
                    ty: 1,
                    offset: *offset,
                    index,
                    addend: 0,
                });
            }
        }
    }

    for reloc in &scan.pending_data {
        if let PendingReloc::DataAddr {
            offset,
            segment_index,
        } = reloc
            && let Some(index) = maps.data_symbol_map.get(*segment_index as usize)
        {
            data_entries.push(RelocEntry {
                ty: 4,
                offset: *offset,
                index: *index,
                addend: 0,
            });
        }
    }

    for reloc in &scan.pending_elem {
        if let PendingReloc::Function { offset, func_index } = reloc
            && let Some(index) = maps.func_symbol_map.get(*func_index as usize)
        {
            elem_entries.push(RelocEntry {
                ty: 0,
                offset: *offset,
                index: *index,
                addend: 0,
            });
        }
    }

    code_entries.sort_by_key(|entry| entry.offset);
    data_entries.sort_by_key(|entry| entry.offset);
    elem_entries.sort_by_key(|entry| entry.offset);

    RelocEntries {
        code: code_entries,
        data: data_entries,
        elem: elem_entries,
    }
}
