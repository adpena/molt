use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::Encode;

use crate::model::{
    DecodedCallableTableAttestation, WasmCallableTableEntryFact, WasmFunctionType, WasmLinkFacts,
};
use crate::{CALLABLE_TABLE_SECTION_VERSION, CALLABLE_TABLE_VALUE_TYPE_FORMAT};

pub(crate) fn encode_callable_table_attestation(facts: &WasmLinkFacts) -> Result<Vec<u8>, String> {
    let used_type_indices = facts
        .callable_table_entries
        .iter()
        .map(|entry| entry.type_index)
        .collect::<BTreeSet<_>>();
    let mut payload = Vec::new();
    CALLABLE_TABLE_SECTION_VERSION.encode(&mut payload);
    CALLABLE_TABLE_VALUE_TYPE_FORMAT.encode(&mut payload);
    u32::try_from(used_type_indices.len())
        .map_err(|_| "callable-table type count exceeds u32")?
        .encode(&mut payload);
    for type_index in used_type_indices {
        let function_type = facts
            .function_types
            .get(usize::try_from(type_index).map_err(|_| "type index exceeds host usize")?)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("callable-table entry references missing type {type_index}"))?;
        type_index.encode(&mut payload);
        encode_attested_value_types(&function_type.params, &mut payload)?;
        encode_attested_value_types(&function_type.results, &mut payload)?;
    }
    u32::try_from(facts.callable_table_entries.len())
        .map_err(|_| "callable-table entry count exceeds u32")?
        .encode(&mut payload);
    let mut prior_slot = 0u32;
    for entry in &facts.callable_table_entries {
        entry
            .slot
            .checked_sub(prior_slot)
            .ok_or("callable-table entries are not slot-sorted")?
            .encode(&mut payload);
        entry.function_index.encode(&mut payload);
        entry.type_index.encode(&mut payload);
        entry.role.encode(&mut payload);
        prior_slot = entry.slot;
    }
    Ok(payload)
}

fn encode_attested_value_types(
    value_types: &[Vec<u8>],
    payload: &mut Vec<u8>,
) -> Result<(), String> {
    u32::try_from(value_types.len())
        .map_err(|_| "callable-table value-type count exceeds u32")?
        .encode(payload);
    for value_type in value_types {
        if value_type.is_empty() {
            return Err("callable-table value type is empty".to_string());
        }
        u32::try_from(value_type.len())
            .map_err(|_| "callable-table value-type width exceeds u32")?
            .encode(payload);
        payload.extend_from_slice(value_type);
    }
    Ok(())
}

pub(crate) fn read_u32_leb(data: &[u8], offset: &mut usize) -> Result<u32, String> {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        let byte = *data
            .get(*offset)
            .ok_or("truncated callable-table attestation varuint")?;
        *offset = offset
            .checked_add(1)
            .ok_or("callable-table attestation offset overflow")?;
        if shift == 28 && byte & 0xF0 != 0 {
            return Err("callable-table attestation varuint exceeds u32".to_string());
        }
        result |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift = shift
            .checked_add(7)
            .ok_or("callable-table attestation varuint shift overflow")?;
        if shift >= 35 {
            return Err("callable-table attestation varuint exceeds u32".to_string());
        }
    }
}

fn read_encoded_value_types(data: &[u8], offset: &mut usize) -> Result<Vec<Vec<u8>>, String> {
    let count = read_u32_leb(data, offset)?;
    let mut values = Vec::with_capacity(
        usize::try_from(count).map_err(|_| "value-type count exceeds host usize")?,
    );
    for _ in 0..count {
        let byte_count = read_u32_leb(data, offset)?;
        if byte_count == 0 {
            return Err("callable-table attestation has empty value type".to_string());
        }
        let byte_count =
            usize::try_from(byte_count).map_err(|_| "value-type width exceeds host usize")?;
        let end = offset
            .checked_add(byte_count)
            .ok_or("callable-table value-type boundary overflow")?;
        let encoded = data
            .get(*offset..end)
            .ok_or("truncated callable-table attestation value type")?;
        values.push(encoded.to_vec());
        *offset = end;
    }
    Ok(values)
}

pub(crate) fn decode_callable_table_attestation(
    data: &[u8],
) -> Result<DecodedCallableTableAttestation, String> {
    let mut offset = 0usize;
    if read_u32_leb(data, &mut offset)? != 1 {
        return Err("unsupported callable-table attestation version".to_string());
    }
    if read_u32_leb(data, &mut offset)? != 1 {
        return Err("unsupported callable-table value-type format".to_string());
    }
    let type_count = read_u32_leb(data, &mut offset)?;
    let mut types = BTreeMap::new();
    let mut previous_type_index = None;
    for _ in 0..type_count {
        let type_index = read_u32_leb(data, &mut offset)?;
        if previous_type_index.is_some_and(|previous| type_index <= previous) {
            return Err("callable-table type indices are not strictly ordered".to_string());
        }
        previous_type_index = Some(type_index);
        let params = read_encoded_value_types(data, &mut offset)?;
        let results = read_encoded_value_types(data, &mut offset)?;
        types.insert(type_index, (params, results));
    }
    let entry_count = read_u32_leb(data, &mut offset)?;
    let mut entries = Vec::with_capacity(
        usize::try_from(entry_count).map_err(|_| "entry count exceeds host usize")?,
    );
    let mut slot = 0u32;
    for entry_index in 0..entry_count {
        let delta = read_u32_leb(data, &mut offset)?;
        if entry_index != 0 && delta == 0 {
            return Err(format!("callable-table attestation duplicates slot {slot}"));
        }
        slot = slot
            .checked_add(delta)
            .ok_or("callable-table attestation slot overflow")?;
        let function_index = read_u32_leb(data, &mut offset)?;
        let type_index = read_u32_leb(data, &mut offset)?;
        let role = read_u32_leb(data, &mut offset)?;
        if role != 0 {
            return Err(format!(
                "callable-table slot {slot} has unknown role {role}"
            ));
        }
        if !types.contains_key(&type_index) {
            return Err(format!(
                "callable-table slot {slot} references missing type"
            ));
        }
        entries.push(WasmCallableTableEntryFact {
            slot,
            function_index,
            type_index,
            role,
        });
    }
    if offset != data.len() {
        return Err("callable-table attestation has trailing bytes".to_string());
    }
    let used_types = entries
        .iter()
        .map(|entry| entry.type_index)
        .collect::<BTreeSet<_>>();
    if types.keys().copied().collect::<BTreeSet<_>>() != used_types {
        return Err("callable-table attestation contains unused types".to_string());
    }
    Ok(DecodedCallableTableAttestation { types, entries })
}

pub(crate) fn validate_callable_table_attestation(
    attestation: DecodedCallableTableAttestation,
    expected_entries: &[WasmCallableTableEntryFact],
    canonical_types: &[Option<WasmFunctionType>],
) -> Result<(), String> {
    if attestation.entries != expected_entries {
        return Err("callable-table attestation disagrees with final module facts".to_string());
    }
    for (type_index, (params, results)) in attestation.types {
        let canonical = canonical_types
            .get(usize::try_from(type_index).map_err(|_| "type index exceeds host usize")?)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                format!("callable-table attestation references non-function type {type_index}")
            })?;
        if canonical.params != params || canonical.results != results {
            return Err(format!(
                "callable-table attestation type {type_index} disagrees with final module type"
            ));
        }
    }
    Ok(())
}
