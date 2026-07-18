use std::collections::BTreeSet;

use wasm_encoder::Encode;

use crate::model::{WasmCallableTableEntryFact, WasmFunctionType, WasmLinkFacts};
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

pub(crate) struct AttestationDecoder<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> AttestationDecoder<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, String> {
        let mut result = 0u32;
        for shift in (0..=28).step_by(7) {
            let byte = *self
                .data
                .get(self.offset)
                .ok_or("truncated callable-table attestation varuint")?;
            self.offset = self
                .offset
                .checked_add(1)
                .ok_or("callable-table attestation offset overflow")?;
            if shift == 28 && byte & 0xF0 != 0 {
                return Err("callable-table attestation varuint exceeds u32".to_string());
            }
            result |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err("callable-table attestation varuint exceeds u32".to_string())
    }

    pub(crate) fn finish(self, label: &str) -> Result<(), String> {
        if self.offset == self.data.len() {
            Ok(())
        } else {
            Err(format!("{label} has trailing bytes"))
        }
    }

    fn read_count(&mut self, label: &str, minimum_item_width: usize) -> Result<usize, String> {
        let count = usize::try_from(self.read_u32()?)
            .map_err(|_| format!("{label} count exceeds host usize"))?;
        let remaining = self
            .data
            .len()
            .checked_sub(self.offset)
            .ok_or("callable-table attestation cursor exceeds payload")?;
        if count > remaining / minimum_item_width {
            return Err(format!(
                "callable-table attestation {label} count {count} exceeds the encoded payload bound {}",
                remaining / minimum_item_width
            ));
        }
        Ok(count)
    }

    fn read_bytes(&mut self, byte_count: usize, label: &str) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(byte_count)
            .ok_or_else(|| format!("callable-table {label} boundary overflow"))?;
        let encoded = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| format!("truncated callable-table attestation {label}"))?;
        self.offset = end;
        Ok(encoded)
    }
}

fn validate_encoded_value_types(
    decoder: &mut AttestationDecoder<'_>,
    expected: &[Vec<u8>],
    label: &str,
) -> Result<(), String> {
    // Every encoded value type has at least a one-byte width and one data byte.
    let count = decoder.read_count("value-type", 2)?;
    if count != expected.len() {
        return Err(format!(
            "callable-table attestation {label} count {count} disagrees with final module count {}",
            expected.len()
        ));
    }
    for expected_type in expected {
        let byte_count = usize::try_from(decoder.read_u32()?)
            .map_err(|_| "value-type width exceeds host usize")?;
        if byte_count == 0 {
            return Err("callable-table attestation has empty value type".to_string());
        }
        if decoder.read_bytes(byte_count, "value type")? != expected_type {
            return Err(format!(
                "callable-table attestation {label} disagrees with final module type"
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_callable_table_attestation(
    data: &[u8],
    expected_entries: &[WasmCallableTableEntryFact],
    canonical_types: &[Option<WasmFunctionType>],
) -> Result<(), String> {
    let mut decoder = AttestationDecoder::new(data);
    if decoder.read_u32()? != 1 {
        return Err("unsupported callable-table attestation version".to_string());
    }
    if decoder.read_u32()? != 1 {
        return Err("unsupported callable-table value-type format".to_string());
    }
    let used_types = expected_entries
        .iter()
        .map(|entry| entry.type_index)
        .collect::<BTreeSet<_>>();
    // A type row contains at least an index and two empty-list counts. Validate
    // directly against the canonical scan so malformed input never drives an
    // allocation proportional to an attested count or width.
    let type_count = decoder.read_count("type", 3)?;
    if type_count != used_types.len() {
        return Err(format!(
            "callable-table attestation type count {type_count} disagrees with final module count {}",
            used_types.len()
        ));
    }
    for expected_type_index in used_types {
        let type_index = decoder.read_u32()?;
        if type_index != expected_type_index {
            return Err(format!(
                "callable-table attestation expected type {expected_type_index}, found {type_index}"
            ));
        }
        let canonical = canonical_types
            .get(usize::try_from(type_index).map_err(|_| "type index exceeds host usize")?)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                format!("callable-table attestation references non-function type {type_index}")
            })?;
        validate_encoded_value_types(&mut decoder, &canonical.params, "parameter")?;
        validate_encoded_value_types(&mut decoder, &canonical.results, "result")?;
    }
    // Every entry contains four varuints, each at least one byte.
    let entry_count = decoder.read_count("entry", 4)?;
    if entry_count != expected_entries.len() {
        return Err(format!(
            "callable-table attestation entry count {entry_count} disagrees with final module count {}",
            expected_entries.len()
        ));
    }
    let mut slot = 0u32;
    for (entry_index, expected) in expected_entries.iter().enumerate() {
        let delta = decoder.read_u32()?;
        if entry_index != 0 && delta == 0 {
            return Err(format!("callable-table attestation duplicates slot {slot}"));
        }
        slot = slot
            .checked_add(delta)
            .ok_or("callable-table attestation slot overflow")?;
        let function_index = decoder.read_u32()?;
        let type_index = decoder.read_u32()?;
        let role = decoder.read_u32()?;
        if role != 0 {
            return Err(format!(
                "callable-table slot {slot} has unknown role {role}"
            ));
        }
        if (slot, function_index, type_index, role)
            != (
                expected.slot,
                expected.function_index,
                expected.type_index,
                expected.role,
            )
        {
            return Err(format!(
                "callable-table attestation slot {slot} disagrees with final module facts"
            ));
        }
    }
    decoder.finish("callable-table attestation")?;
    Ok(())
}
