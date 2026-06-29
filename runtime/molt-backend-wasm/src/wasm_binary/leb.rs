pub(super) fn read_u32_leb128(data: &[u8], mut offset: usize) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(offset)?;
        offset += 1;
        if shift >= 32 && (byte & 0x7f) != 0 {
            return None;
        }
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 35 {
            return None;
        }
    }
    Some((result, offset))
}

pub(super) fn encode_u32_leb128(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}
