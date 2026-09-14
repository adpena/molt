use super::*;

pub(super) fn detect_wasm_table_base(source: &ModuleSource) -> Result<Option<u64>> {
    let path = source.path();
    let facts = molt_wasm_facts::scan_wasm_link_facts(source.bytes())
        .map_err(wasmtime::Error::msg)
        .with_context(|| format!("decode wasm table-base facts from {path:?}"))?;
    let active_table_bases = facts
        .active_element_segments
        .iter()
        .filter(|segment| segment.base > 0)
        .map(|segment| u64::from(segment.base));
    Ok(active_table_bases
        .clone()
        .filter(|base| *base > 1)
        .min()
        .or_else(|| active_table_bases.min()))
}
