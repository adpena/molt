//! Minimal WTF-8 output helpers shared by text codec tables.

pub fn push_wtf8_codepoint(out: &mut Vec<u8>, code: u32) {
    if code <= 0x7F {
        out.push(code as u8);
    } else if code <= 0x7FF {
        out.push(0xC0 | ((code >> 6) as u8));
        out.push(0x80 | (code as u8 & 0x3F));
    } else if code <= 0xFFFF {
        out.push(0xE0 | ((code >> 12) as u8));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    } else {
        out.push(0xF0 | ((code >> 18) as u8));
        out.push(0x80 | (((code >> 12) as u8) & 0x3F));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    }
}

pub fn push_backslash_bytes_vec(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push(b'\\');
        out.push(b'x');
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
}
