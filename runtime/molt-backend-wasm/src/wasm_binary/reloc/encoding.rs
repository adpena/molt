use std::borrow::Cow;

use wasm_encoder::{CustomSection, Encode};

use super::types::RelocEntry;

pub(super) fn encode_reloc_section(
    name: &'static str,
    section_index: u32,
    entries: &[RelocEntry],
) -> CustomSection<'static> {
    let mut data = Vec::new();
    section_index.encode(&mut data);
    (entries.len() as u32).encode(&mut data);
    for entry in entries {
        data.push(entry.ty);
        entry.offset.encode(&mut data);
        entry.index.encode(&mut data);
        if matches!(entry.ty, 4 | 5) {
            entry.addend.encode(&mut data);
        }
    }
    CustomSection {
        name: name.into(),
        data: Cow::Owned(data),
    }
}

pub(super) fn append_custom_section(bytes: &mut Vec<u8>, section: &impl Encode) {
    bytes.push(0);
    section.encode(bytes);
}
