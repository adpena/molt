use std::io::Write;

use crate::encoding::encode_callable_table_attestation;
use crate::layout::{encode_callable_table_layout, validate_callable_table_layout};
use crate::model::*;
use crate::scan::scan_wasm_link_facts_with_sections;
use crate::{CALLABLE_TABLE_LAYOUT_SECTION_NAME, CALLABLE_TABLE_SECTION_NAME};
use wasm_encoder::Encode;

pub fn publish_callable_table_attestation(
    bytes: &[u8],
    layout: Option<CallableTableLayout>,
) -> Result<Vec<u8>, String> {
    let mut published = Vec::with_capacity(bytes.len());
    scan_and_write_callable_table_attestation(
        bytes,
        layout,
        CallableTableArtifactRole::Monolithic,
        &mut published,
    )?;
    Ok(published)
}

pub fn scan_and_write_callable_table_attestation(
    bytes: &[u8],
    layout: Option<CallableTableLayout>,
    role: CallableTableArtifactRole,
    writer: &mut impl Write,
) -> Result<WasmLinkFacts, String> {
    writer
        .write_all(b"\0asm\x01\0\0\0")
        .map_err(|error| error.to_string())?;
    let mut facts = {
        let mut emit_section = |id, payload: &[u8]| write_raw_section(writer, id, payload);
        scan_wasm_link_facts_with_sections(bytes, Some(&mut emit_section))?
    };
    validate_callable_table_topology(&facts)?;
    if !facts.forbidden_callable_alias_exports.is_empty() {
        return Err(format!(
            "final wasm contains forbidden numeric callable-table alias(es): {}",
            facts.forbidden_callable_alias_exports.join(", ")
        ));
    }
    let entries = &facts.callable_table_entries;
    // Final active elements are the only artifact-local entry-count authority.
    // A pre-link app layout section can survive wasm-ld and optimization, but
    // native objects add callable entries during the final link. Preserve the
    // executable-runtime boundary while replacing the stale count from the
    // final app itself; the runtime then consumes that published final layout.
    let layout = match (layout, role) {
        (Some(mut layout), CallableTableArtifactRole::App) => {
            layout.app_entry_count = u32::try_from(entries.len())
                .map_err(|_| "callable-table entry count exceeds u32")?;
            layout
        }
        (Some(layout), _) => layout,
        (None, CallableTableArtifactRole::Monolithic) => CallableTableLayout {
            fixed_prefix_base: 0,
            fixed_prefix_len: 0,
            finalized_app_base: entries.first().map_or(0, |entry| entry.slot),
            app_entry_count: u32::try_from(entries.len())
                .map_err(|_| "callable-table entry count exceeds u32")?,
        },
        (None, CallableTableArtifactRole::App | CallableTableArtifactRole::Runtime) => {
            return Err(
                "split callable-table publication requires an explicit final layout".into(),
            );
        }
    };
    validate_callable_table_layout(layout, entries, role)?;
    let attestation = encode_callable_table_attestation(&facts)?;
    let layout_payload = encode_callable_table_layout(layout);
    write_custom_section(writer, CALLABLE_TABLE_LAYOUT_SECTION_NAME, &layout_payload)?;
    write_custom_section(writer, CALLABLE_TABLE_SECTION_NAME, &attestation)?;
    facts.callable_table_attestation_present = true;
    facts.callable_table_layout = Some(layout);
    Ok(facts)
}

fn validate_callable_table_topology(facts: &WasmLinkFacts) -> Result<(), String> {
    if !facts.callable_table_entries.is_empty() {
        match facts.tables.first() {
            Some(table) if table.table_index == 0 && table.untyped_funcref => {}
            Some(table) => {
                return Err(format!(
                    "callable-table entries require table 0 to be untyped funcref, found element type {:?}",
                    table.encoded_element_type
                ));
            }
            None => return Err("callable-table entries require table 0".to_string()),
        }
    }
    if let Some(element) = facts.active_function_elements.iter().find(|element| {
        element.table_index != 0
            && (facts.exported_table_indices.contains(&element.table_index)
                || facts
                    .reachable_indirect_call_tables
                    .contains(&element.table_index))
    }) {
        return Err(format!(
            "active callable function {} escapes canonical table 0 through table {} slot {}",
            element.function_index, element.table_index, element.slot
        ));
    }
    if let Some(table_index) = facts
        .reachable_indirect_call_tables
        .iter()
        .copied()
        .find(|table_index| *table_index != 0)
    {
        return Err(format!(
            "indirect callable dispatch escapes canonical table 0 through table {table_index}"
        ));
    }
    if facts.reachable_function_reference_dispatch
        && let Some(read) = facts
            .reachable_table_reads
            .iter()
            .find(|read| read.table_index != 0)
    {
        return Err(format!(
            "function-reference dispatch can escape canonical table 0 through reachable table.get {} in function {}",
            read.table_index, read.function_index
        ));
    }
    if let Some(mutation) = facts
        .reachable_table_mutations
        .iter()
        .find(|mutation| mutation.table_index == 0 || mutation.source_table_index == Some(0))
    {
        let source = mutation
            .source_table_index
            .map_or_else(String::new, |table| format!(", source table {table}"));
        return Err(format!(
            "final wasm mutates or escapes callable table 0 via {} in function {} (destination table {}{source})",
            mutation.operation, mutation.function_index, mutation.table_index
        ));
    }
    Ok(())
}

fn write_raw_section(writer: &mut impl Write, id: u8, payload: &[u8]) -> Result<(), String> {
    writer.write_all(&[id]).map_err(|error| error.to_string())?;
    let mut encoded_len = Vec::with_capacity(5);
    u32::try_from(payload.len())
        .map_err(|_| "wasm section payload exceeds u32")?
        .encode(&mut encoded_len);
    writer
        .write_all(&encoded_len)
        .and_then(|()| writer.write_all(payload))
        .map_err(|error| error.to_string())
}

fn write_custom_section(writer: &mut impl Write, name: &str, payload: &[u8]) -> Result<(), String> {
    let mut custom_payload = Vec::with_capacity(name.len() + payload.len() + 5);
    u32::try_from(name.len())
        .map_err(|_| "custom section name exceeds u32")?
        .encode(&mut custom_payload);
    custom_payload.extend_from_slice(name.as_bytes());
    custom_payload.extend_from_slice(payload);
    write_raw_section(writer, 0, &custom_payload)
}
