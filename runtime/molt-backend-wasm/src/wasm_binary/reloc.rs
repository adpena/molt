use crate::wasm_data::{DataRelocSite, DataSegmentInfo};
use crate::wasm_table::TableRelocSite;

use std::borrow::Cow;

use wasm_encoder::{CustomSection, Encode, SymbolTable};

mod encoding;
mod resolve;
mod scan;
mod symbols;
mod types;

#[cfg(test)]
mod tests;

use encoding::{append_custom_section, encode_reloc_section};
use resolve::resolve_reloc_entries;
use scan::RelocScan;
use symbols::build_symbol_maps;

pub(crate) fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
    table_relocs: &[TableRelocSite],
) -> Vec<u8> {
    let mut scan = match RelocScan::collect(&bytes) {
        Some(scan) => scan,
        None => return bytes,
    };

    let code_section_start = match scan.code_section_start {
        Some(start) => start,
        None => return bytes,
    };
    let code_section_index = match scan.code_section_index {
        Some(index) => index,
        None => return bytes,
    };

    scan.record_data_reloc_sites(&bytes, data_relocs, code_section_start);
    scan.record_table_reloc_sites(&bytes, table_relocs, code_section_start);

    let symbol_maps = build_symbol_maps(&scan, data_segments);
    let entries = resolve_reloc_entries(&scan, &symbol_maps);

    append_custom_section(
        &mut bytes,
        &linking_section(&symbol_maps.sym_tab, data_segments),
    );
    if !entries.code.is_empty() {
        let reloc_code = encode_reloc_section("reloc.CODE", code_section_index, &entries.code);
        append_custom_section(&mut bytes, &reloc_code);
    }
    if !entries.data.is_empty()
        && let Some(index) = scan.data_section_index
    {
        let reloc_data = encode_reloc_section("reloc.DATA", index, &entries.data);
        append_custom_section(&mut bytes, &reloc_data);
    }
    if !entries.elem.is_empty()
        && let Some(index) = scan.element_section_index
    {
        let reloc_elem = encode_reloc_section("reloc.ELEM", index, &entries.elem);
        append_custom_section(&mut bytes, &reloc_elem);
    }

    bytes
}

/// The `linking` custom section: the symbol table, then the segment info
/// subsection (`WASM_SEGMENT_INFO`), which carries each data segment's
/// alignment so the linker keeps the placement the backend chose
/// (`wasm_data::data_segment_align_mask`): a static module cell block's base
/// travels shifted right by 3 (call_anatomy.md 4.5a) and must stay 8-aligned.
/// `wasm-encoder` has no segment info builder yet, so this encodes both.
fn linking_section(
    symbols: &SymbolTable,
    data_segments: &[DataSegmentInfo],
) -> CustomSection<'static> {
    const LINKING_VERSION: u32 = 2;
    const WASM_SEGMENT_INFO: u8 = 5;
    let mut data = Vec::new();
    LINKING_VERSION.encode(&mut data);
    symbols.encode(&mut data);
    let mut info = Vec::new();
    (data_segments.len() as u32).encode(&mut info);
    for (index, segment) in data_segments.iter().enumerate() {
        format!(".data.__molt_output_data_{index}").encode(&mut info);
        crate::wasm_data::data_segment_align_log2(segment.size).encode(&mut info);
        0u32.encode(&mut info);
    }
    data.push(WASM_SEGMENT_INFO);
    info.as_slice().encode(&mut data);
    CustomSection {
        name: "linking".into(),
        data: Cow::Owned(data),
    }
}
