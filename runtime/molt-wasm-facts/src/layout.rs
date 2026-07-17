use wasm_encoder::Encode;

use crate::CALLABLE_TABLE_LAYOUT_VERSION;
use crate::encoding::read_u32_leb;
use crate::model::{CallableTableArtifactRole, CallableTableLayout, WasmCallableTableEntryFact};

pub(crate) fn validate_callable_table_layout(
    layout: CallableTableLayout,
    entries: &[WasmCallableTableEntryFact],
    role: CallableTableArtifactRole,
) -> Result<(), String> {
    let fixed_end = layout
        .fixed_prefix_base
        .checked_add(layout.fixed_prefix_len)
        .ok_or("callable-table fixed prefix overflows u32")?;
    let app_end = layout
        .finalized_app_base
        .checked_add(layout.app_entry_count)
        .ok_or("callable-table app region overflows u32")?;
    if fixed_end > layout.finalized_app_base {
        return Err("callable-table fixed and app regions overlap".to_string());
    }
    if role == CallableTableArtifactRole::Monolithic && layout.fixed_prefix_len != 0 {
        let fixed_count = usize::try_from(layout.fixed_prefix_len)
            .map_err(|_| "callable-table fixed prefix exceeds host usize")?;
        let app_count = usize::try_from(layout.app_entry_count)
            .map_err(|_| "callable-table app region exceeds host usize")?;
        let expected_count = fixed_count
            .checked_add(app_count)
            .ok_or("callable-table monolithic entry count exceeds host usize")?;
        if entries.len() != expected_count {
            return Err(format!(
                "callable-table monolithic entry count does not match fixed+app layout: layout={expected_count}, final={}",
                entries.len()
            ));
        }
        validate_contiguous_entries(
            &entries[..fixed_count],
            layout.fixed_prefix_base,
            "runtime fixed prefix",
        )?;
        validate_contiguous_entries(&entries[fixed_count..], layout.finalized_app_base, "app")?;
        return Ok(());
    }
    let (expected_base, expected_count, label) = match role {
        CallableTableArtifactRole::Monolithic | CallableTableArtifactRole::App => {
            (layout.finalized_app_base, layout.app_entry_count, "app")
        }
        CallableTableArtifactRole::Runtime => (
            layout.fixed_prefix_base,
            layout.fixed_prefix_len,
            "runtime fixed prefix",
        ),
    };
    if usize::try_from(expected_count).ok() != Some(entries.len()) {
        return Err(format!(
            "callable-table layout {label} entry count does not match final active elements: layout={expected_count}, final={}",
            entries.len()
        ));
    }
    validate_contiguous_entries(entries, expected_base, label)?;
    let _ = app_end;
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
    layout.fixed_prefix_base.encode(&mut payload);
    layout.fixed_prefix_len.encode(&mut payload);
    layout.finalized_app_base.encode(&mut payload);
    layout.app_entry_count.encode(&mut payload);
    payload
}

pub(crate) fn decode_callable_table_layout(payload: &[u8]) -> Result<CallableTableLayout, String> {
    let mut offset = 0usize;
    if read_u32_leb(payload, &mut offset)? != CALLABLE_TABLE_LAYOUT_VERSION {
        return Err("unsupported callable-table layout version".to_string());
    }
    let layout = CallableTableLayout {
        fixed_prefix_base: read_u32_leb(payload, &mut offset)?,
        fixed_prefix_len: read_u32_leb(payload, &mut offset)?,
        finalized_app_base: read_u32_leb(payload, &mut offset)?,
        app_entry_count: read_u32_leb(payload, &mut offset)?,
    };
    if offset != payload.len() {
        return Err("callable-table layout has trailing bytes".to_string());
    }
    layout
        .fixed_prefix_base
        .checked_add(layout.fixed_prefix_len)
        .filter(|fixed_end| *fixed_end <= layout.finalized_app_base)
        .ok_or("callable-table layout fixed prefix overlaps app region")?;
    layout
        .finalized_app_base
        .checked_add(layout.app_entry_count)
        .ok_or("callable-table layout app region overflows u32")?;
    Ok(layout)
}
