use wasm_encoder::Encode;

use crate::CALLABLE_TABLE_LAYOUT_VERSION;
use crate::encoding::AttestationDecoder;
use crate::model::{CallableTableArtifactRole, CallableTableLayout, WasmCallableTableEntryFact};

/// The layout names the app's region only. Runtime callables are keys, so no
/// table region is reserved for them (HF-699 K3); the runtime's own entries
/// sit below the app base.
pub(crate) fn validate_callable_table_layout(
    layout: CallableTableLayout,
    entries: &[WasmCallableTableEntryFact],
    role: CallableTableArtifactRole,
) -> Result<(), String> {
    layout
        .finalized_app_base
        .checked_add(layout.app_entry_count)
        .ok_or("callable-table app region overflows u32")?;
    match role {
        CallableTableArtifactRole::Monolithic => {
            let app_start = entries.partition_point(|entry| entry.slot < layout.finalized_app_base);
            let (runtime_entries, app_entries) = entries.split_at(app_start);
            validate_runtime_entries(layout, runtime_entries)?;
            validate_app_entries(layout, app_entries)
        }
        CallableTableArtifactRole::App => {
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.slot < layout.finalized_app_base)
            {
                return Err(format!(
                    "split app publishes runtime-owned callable slot {} below app base {}",
                    entry.slot, layout.finalized_app_base
                ));
            }
            validate_app_entries(layout, entries)
        }
        CallableTableArtifactRole::Runtime => validate_runtime_entries(layout, entries),
    }
}

fn validate_app_entries(
    layout: CallableTableLayout,
    entries: &[WasmCallableTableEntryFact],
) -> Result<(), String> {
    let expected_app_count = usize::try_from(layout.app_entry_count)
        .map_err(|_| "callable-table app region exceeds host usize")?;
    if entries.len() != expected_app_count {
        return Err(format!(
            "callable-table app entry count does not match final active elements: layout={expected_app_count}, final={}",
            entries.len()
        ));
    }
    validate_contiguous_entries(entries, layout.finalized_app_base, "app")
}

fn validate_runtime_entries(
    layout: CallableTableLayout,
    entries: &[WasmCallableTableEntryFact],
) -> Result<(), String> {
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.slot >= layout.finalized_app_base)
    {
        return Err(format!(
            "callable-table runtime slot reaches finalized app base: {} >= {}",
            entry.slot, layout.finalized_app_base
        ));
    }
    Ok(())
}

fn validate_contiguous_entries(
    entries: &[WasmCallableTableEntryFact],
    expected_base: u32,
    label: &str,
) -> Result<(), String> {
    for (offset, entry) in entries.iter().enumerate() {
        let expected_slot = expected_base
            .checked_add(u32::try_from(offset).map_err(|_| "callable-table offset exceeds u32")?)
            .ok_or("callable-table expected slot overflows u32")?;
        if entry.slot != expected_slot {
            return Err(format!(
                "callable-table {label} publication is not contiguous: expected slot {expected_slot}, found {}",
                entry.slot
            ));
        }
    }
    Ok(())
}

pub(crate) fn encode_callable_table_layout(layout: CallableTableLayout) -> Vec<u8> {
    let mut payload = Vec::new();
    CALLABLE_TABLE_LAYOUT_VERSION.encode(&mut payload);
    layout.finalized_app_base.encode(&mut payload);
    layout.app_entry_count.encode(&mut payload);
    payload
}

pub(crate) fn decode_callable_table_layout(payload: &[u8]) -> Result<CallableTableLayout, String> {
    let mut decoder = AttestationDecoder::new(payload);
    if decoder.read_u32()? != CALLABLE_TABLE_LAYOUT_VERSION {
        return Err("unsupported callable-table layout version".to_string());
    }
    let layout = CallableTableLayout {
        finalized_app_base: decoder.read_u32()?,
        app_entry_count: decoder.read_u32()?,
    };
    decoder.finish("callable-table layout")?;
    layout
        .finalized_app_base
        .checked_add(layout.app_entry_count)
        .ok_or("callable-table layout app region overflows u32")?;
    Ok(layout)
}
