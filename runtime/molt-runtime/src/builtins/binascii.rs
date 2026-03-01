use crate::*;

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Hardware-accelerated CRC32 on AArch64 using the CRC extension instructions.
/// Apple Silicon (M1/M2/M3/M4) always supports these. Processes 8 bytes per
/// iteration using `__crc32d`, falling back to per-byte `__crc32b` for remainder.
#[cfg(target_arch = "aarch64")]
unsafe fn crc32_hw_aarch64(data: &[u8]) -> u32 {
    unsafe {
        use std::arch::aarch64::*;
        let mut crc = 0xFFFF_FFFFu32;
        let mut i = 0usize;
        // Process 8 bytes at a time using CRC32 doubleword instruction
        while i + 8 <= data.len() {
            let chunk = u64::from_le_bytes([
                data[i], data[i + 1], data[i + 2], data[i + 3],
                data[i + 4], data[i + 5], data[i + 6], data[i + 7],
            ]);
            crc = __crc32d(crc, chunk);
            i += 8;
        }
        // Process remaining bytes one at a time
        while i < data.len() {
            crc = __crc32b(crc, data[i]);
            i += 1;
        }
        crc ^ 0xFFFF_FFFF
    }
}

fn bytes_like_arg(_py: &PyToken<'_>, bits: u64, func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{func}() argument 1 must be bytes-like, not None");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
        let type_name = type_name(_py, obj);
        let msg = format!("a bytes-like object is required, not '{type_name}'");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    Ok(raw.to_vec())
}

fn ascii_or_bytes_arg(_py: &PyToken<'_>, bits: u64, _func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(text.into_bytes());
    }
    bytes_like_arg(_py, bits, "a2b")
}

fn alloc_bytes_or_oom(_py: &PyToken<'_>, data: &[u8], context: &str) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        let msg = format!("{context}: out of memory");
        return raise_exception::<_>(_py, "MemoryError", &msg);
    }
    MoltObject::from_ptr(ptr).bits()
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !(*b as char).is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if !compact.len().is_multiple_of(4) {
        return Err("Incorrect padding");
    }
    let mut out = Vec::with_capacity(compact.len() / 4 * 3);
    let mut idx = 0usize;
    while idx < compact.len() {
        let c0 = compact[idx];
        let c1 = compact[idx + 1];
        let c2 = compact[idx + 2];
        let c3 = compact[idx + 3];

        let Some(v0) = base64_value(c0) else {
            return Err("Non-base64 digit found");
        };
        let Some(v1) = base64_value(c1) else {
            return Err("Non-base64 digit found");
        };
        let pad2 = c2 == b'=';
        let pad3 = c3 == b'=';
        let v2 = if pad2 {
            0
        } else if let Some(v) = base64_value(c2) {
            v
        } else {
            return Err("Non-base64 digit found");
        };
        let v3 = if pad3 {
            0
        } else if let Some(v) = base64_value(c3) {
            v
        } else {
            return Err("Non-base64 digit found");
        };
        if pad2 && !pad3 {
            return Err("Incorrect padding");
        }

        out.push((v0 << 2) | (v1 >> 4));
        if !pad2 {
            out.push(((v1 & 0x0F) << 4) | (v2 >> 2));
        }
        if !pad3 {
            out.push(((v2 & 0x03) << 6) | v3);
        }
        idx += 4;
    }
    Ok(out)
}

fn base64_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4 + 1);
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(BASE64_TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(BASE64_TABLE[((n >> 12) & 0x3f) as usize]);
        if idx + 1 < input.len() {
            out.push(BASE64_TABLE[((n >> 6) & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        if idx + 2 < input.len() {
            out.push(BASE64_TABLE[(n & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        idx += 3;
    }
    out.push(b'\n');
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn uu_val(ch: u8) -> Option<u8> {
    if ch == b'`' {
        return Some(0);
    }
    if !(b' '..=b'_').contains(&ch) {
        return None;
    }
    Some((ch - b' ') & 0x3f)
}

fn uu_decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut line = input;
    while let Some(last) = line.last() {
        if *last == b'\n' || *last == b'\r' {
            line = &line[..line.len() - 1];
        } else {
            break;
        }
    }
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let Some(want) = uu_val(line[0]) else {
        return Err("Illegal char");
    };
    let want_len = usize::from(want);
    let groups = want_len.div_ceil(3);
    if line.len() < 1 + groups * 4 {
        return Err("Truncated input");
    }
    let mut out = Vec::with_capacity(groups * 3);
    let mut idx = 1usize;
    for _ in 0..groups {
        let Some(a) = uu_val(line[idx]) else {
            return Err("Illegal char");
        };
        let Some(b) = uu_val(line[idx + 1]) else {
            return Err("Illegal char");
        };
        let Some(c) = uu_val(line[idx + 2]) else {
            return Err("Illegal char");
        };
        let Some(d) = uu_val(line[idx + 3]) else {
            return Err("Illegal char");
        };
        out.push((a << 2) | (b >> 4));
        out.push(((b & 0x0f) << 4) | (c >> 2));
        out.push(((c & 0x03) << 6) | d);
        idx += 4;
    }
    out.truncate(want_len);
    Ok(out)
}

fn uu_encode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() > 45 {
        return Err("At most 45 bytes at once");
    }
    let mut out = Vec::with_capacity(2 + input.len().div_ceil(3) * 4);
    out.push(((input.len() as u8) & 0x3f) + b' ');
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let c0 = ((b0 >> 2) & 0x3f) + b' ';
        let c1 = (((b0 << 4) | (b1 >> 4)) & 0x3f) + b' ';
        let c2 = (((b1 << 2) | (b2 >> 6)) & 0x3f) + b' ';
        let c3 = (b2 & 0x3f) + b' ';
        out.extend_from_slice(&[c0, c1, c2, c3]);
        idx += 3;
    }
    out.push(b'\n');
    Ok(out)
}

fn qp_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'=' && idx + 2 < input.len() {
            if input[idx + 1] == b'\r' && input[idx + 2] == b'\n' {
                idx += 3;
                continue;
            }
            if input[idx + 1] == b'\n' {
                idx += 2;
                continue;
            }
            if let (Some(a), Some(b)) = (hex_nibble(input[idx + 1]), hex_nibble(input[idx + 2])) {
                out.push((a << 4) | b);
                idx += 3;
                continue;
            }
        }
        out.push(input[idx]);
        idx += 1;
    }
    out
}

fn qp_encode(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(input.len() * 3 / 2);
    for &b in input {
        if b == b'\n' || b == b'\r' || (b' '..=b'~').contains(&b) && b != b'=' {
            out.push(b);
        } else {
            out.push(b'=');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 0x0f) as usize]);
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_base64(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_base64") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let decoded = match base64_decode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &decoded, "a2b_base64")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_base64(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_base64") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = base64_encode(&raw);
        alloc_bytes_or_oom(_py, &encoded, "b2a_base64")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_hex(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_hex") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        if !raw.len().is_multiple_of(2) {
            return raise_exception::<_>(_py, "ValueError", "Odd-length string");
        }
        let mut out = Vec::with_capacity(raw.len() / 2);
        let mut idx = 0usize;
        while idx < raw.len() {
            let Some(hi) = hex_nibble(raw[idx]) else {
                return raise_exception::<_>(_py, "ValueError", "Non-hexadecimal digit found");
            };
            let Some(lo) = hex_nibble(raw[idx + 1]) else {
                return raise_exception::<_>(_py, "ValueError", "Non-hexadecimal digit found");
            };
            out.push((hi << 4) | lo);
            idx += 2;
        }
        alloc_bytes_or_oom(_py, &out, "a2b_hex")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_hex(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let raw = match bytes_like_arg(_py, data_bits, "b2a_hex") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let mut out = Vec::with_capacity(raw.len() * 2);
        for byte in raw {
            out.push(HEX[(byte >> 4) as usize]);
            out.push(HEX[(byte & 0x0f) as usize]);
        }
        alloc_bytes_or_oom(_py, &out, "b2a_hex")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_qp(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_qp") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = qp_decode(&raw);
        alloc_bytes_or_oom(_py, &out, "a2b_qp")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_qp(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_qp") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = qp_encode(&raw);
        alloc_bytes_or_oom(_py, &out, "b2a_qp")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_uu(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_uu") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = match uu_decode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &out, "a2b_uu")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_uu(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_uu") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = match uu_encode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &out, "b2a_uu")
    })
}

/// CRC32 lookup table (IEEE polynomial 0xEDB88320). 256 entries, generated at
/// compile time. Each entry[i] = CRC32 of byte i, allowing single-lookup per byte
/// instead of 8 conditional branch iterations.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_crc32(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "crc32") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        // Hardware CRC32 on AArch64 (Apple Silicon M1+ always has CRC32 extension)
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("crc") {
                let crc = unsafe { crc32_hw_aarch64(&raw) };
                return MoltObject::from_int(i64::from(crc)).bits();
            }
        }
        // Table-driven CRC32: 1 lookup per byte instead of 8 branches.
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in &raw {
            let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
            crc = (crc >> 8) ^ CRC32_TABLE[idx];
        }
        crc ^= 0xFFFF_FFFF;
        MoltObject::from_int(i64::from(crc)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_crc_hqx(data_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "crc_hqx") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let value = index_i64_from_obj(_py, value_bits, "crc_hqx() arg 2 must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mut crc = (value as u32) & 0xFFFF;
        for byte in raw {
            crc ^= (u32::from(byte)) << 8;
            for _ in 0..8 {
                if (crc & 0x8000) != 0 {
                    crc = ((crc << 1) ^ 0x1021) & 0xFFFF;
                } else {
                    crc = (crc << 1) & 0xFFFF;
                }
            }
        }
        MoltObject::from_int(i64::from(crc as i32)).bits()
    })
}

// ---------------------------------------------------------------------------
// UU codec-level intrinsics (full-message framing around per-line primitives)
// ---------------------------------------------------------------------------

/// `molt_uu_codec_encode(data, filename, mode)` — full UU-encoded message with
/// begin/end framing.  Reuses the internal `uu_encode` per-line function.
#[unsafe(no_mangle)]
pub extern "C" fn molt_uu_codec_encode(
    data_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "uu_codec_encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(s) => s.replace('\n', "\\n").replace('\r', "\\r"),
            None => "<data>".to_string(),
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0o666) & 0o777;

        let mut out = Vec::with_capacity(raw.len() * 2 + 64);
        // begin header
        let header = format!("begin {mode:o} {filename}\n");
        out.extend_from_slice(header.as_bytes());

        // Encode in 45-byte chunks
        let mut offset = 0usize;
        while offset < raw.len() {
            let end = std::cmp::min(offset + 45, raw.len());
            let chunk = &raw[offset..end];
            match uu_encode(chunk) {
                Ok(encoded) => out.extend_from_slice(&encoded),
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            }
            offset = end;
        }

        // Trailer
        out.extend_from_slice(b" \nend\n");

        alloc_bytes_or_oom(_py, &out, "uu_codec_encode")
    })
}

/// `molt_uu_codec_decode(data)` — decode a full UU-encoded message (find begin,
/// decode lines, expect end).
#[unsafe(no_mangle)]
pub extern "C" fn molt_uu_codec_decode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "uu_codec_decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        // Find start of encoded data — scan for line starting with "begin"
        let mut pos = 0usize;
        let mut found_begin = false;
        loop {
            if pos >= raw.len() {
                break;
            }
            // Find end of current line
            let line_end = raw[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| pos + i + 1)
                .unwrap_or(raw.len());
            let line = &raw[pos..line_end];
            if line.len() >= 5 && &line[..5] == b"begin" {
                pos = line_end;
                found_begin = true;
                break;
            }
            pos = line_end;
        }

        if !found_begin {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Missing \"begin\" line in input data",
            );
        }

        // Decode lines until "end" or EOF
        let mut out = Vec::with_capacity(raw.len());
        let mut found_end = false;
        loop {
            if pos >= raw.len() {
                break;
            }
            let line_end = raw[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| pos + i + 1)
                .unwrap_or(raw.len());
            let line = &raw[pos..line_end];
            pos = line_end;

            if line == b"end\n" || line == b"end" {
                found_end = true;
                break;
            }

            // Try normal decode; on error, use broken-uuencoder workaround
            match uu_decode(line) {
                Ok(decoded) => out.extend_from_slice(&decoded),
                Err(_) => {
                    // Workaround for broken uuencoders: use byte-count header
                    if !line.is_empty() {
                        let nbytes =
                            ((((line[0].wrapping_sub(32)) & 63) as usize) * 4 + 5) / 3;
                        let truncated = &line[..std::cmp::min(nbytes, line.len())];
                        if let Ok(decoded) = uu_decode(truncated) {
                            out.extend_from_slice(&decoded);
                        }
                    }
                }
            }
        }

        if !found_end {
            return raise_exception::<_>(_py, "ValueError", "Truncated input data");
        }

        alloc_bytes_or_oom(_py, &out, "uu_codec_decode")
    })
}
