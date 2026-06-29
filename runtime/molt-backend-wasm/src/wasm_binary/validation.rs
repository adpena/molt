use super::leb::read_u32_leb128;

/// Validate that a WASM binary has well-formed section structure.
pub(crate) fn validate_wasm_sections(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    if &bytes[0..4] != b"\x00asm" {
        return false;
    }
    let mut pos = 8usize;
    while pos < bytes.len() {
        let Some(section_id) = bytes.get(pos).copied() else {
            return false;
        };
        if section_id > 13 {
            return false;
        }
        pos += 1;
        let Some((size, content_start)) = read_u32_leb128(bytes, pos) else {
            return false;
        };
        let Some(content_end) = content_start.checked_add(size as usize) else {
            return false;
        };
        if content_end > bytes.len() {
            return false;
        }
        pos = content_end;
    }
    pos == bytes.len()
}
