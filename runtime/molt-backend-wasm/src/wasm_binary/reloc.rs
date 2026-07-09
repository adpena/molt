use crate::wasm_data::{DataRelocSite, DataSegmentInfo};

use wasm_encoder::LinkingSection;

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

    scan.record_data_reloc_sites(data_relocs, code_section_start);

    let symbol_maps = build_symbol_maps(&scan, data_segments);
    let entries = resolve_reloc_entries(&scan, &symbol_maps);

    let mut linking = LinkingSection::new();
    linking.symbol_table(&symbol_maps.sym_tab);
    append_custom_section(&mut bytes, &linking);
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
