use super::*;

pub(super) fn detect_wasm_table_base(path: &Path) -> Result<Option<u64>> {
    let file = fs::File::open(path)
        .with_context(|| format!("open wasm table-base facts input {path:?}"))?;
    // The host consumes an immutable build artifact and retains the descriptor
    // for the scan. Mapping avoids a second whole-module heap copy before
    // Wasmtime opens the same artifact.
    let data = unsafe { memmap2::MmapOptions::new().map(&file) }
        .with_context(|| format!("map wasm table-base facts input {path:?}"))?;
    let facts = molt_wasm_facts::scan_wasm_link_facts(&data)
        .map_err(anyhow::Error::msg)
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
