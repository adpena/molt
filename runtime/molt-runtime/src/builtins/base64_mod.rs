use crate::*;

// ─── Standard base64 alphabet ───────────────────────────────────────────────

const B64_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URLSAFE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

const B32_STD: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const B32_HEX: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";

const B85_ALPHABET: &[u8; 85] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";

const A85_START: &[u8] = b"<~";
const A85_END: &[u8] = b"~>";

const MAXLINESIZE: usize = 76;
const MAXBINSIZE: usize = (MAXLINESIZE / 4) * 3;

// ─── helpers ────────────────────────────────────────────────────────────────

fn bytes_like_arg(_py: &PyToken<'_>, bits: u64, func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "a bytes-like object is required, not 'NoneType'",
        ));
    }
    let Some(ptr) = obj.as_ptr() else {
        let type_label = type_name(_py, obj);
        let msg = format!("a bytes-like object is required, not '{type_label}'");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    // Accept strings — encode to ASCII for decode input
    unsafe {
        if object_type_id(ptr) == TYPE_ID_STRING {
            let len = string_len(ptr);
            let data = string_bytes(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            // Check for non-ASCII
            for &b in slice {
                if b > 127 {
                    return Err(raise_exception::<_>(
                        _py,
                        "ValueError",
                        "string argument should contain only ASCII characters",
                    ));
                }
            }
            return Ok(slice.to_vec());
        }
        if let Some(raw) = bytes_like_slice(ptr) {
            return Ok(raw.to_vec());
        }
    }
    let type_label = type_name(_py, obj);
    let msg = format!(
        "argument should be a bytes-like object or ASCII string, not '{type_label}'"
    );
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

fn encode_bytes_like_arg(_py: &PyToken<'_>, bits: u64, _func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "a bytes-like object is required, not 'NoneType'",
        ));
    }
    let Some(ptr) = obj.as_ptr() else {
        let type_label = type_name(_py, obj);
        let msg = format!("a bytes-like object is required, not '{type_label}'");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    unsafe {
        if let Some(raw) = bytes_like_slice(ptr) {
            return Ok(raw.to_vec());
        }
    }
    let type_label = type_name(_py, obj);
    let msg = format!("a bytes-like object is required, not '{type_label}'");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

fn alloc_bytes_result(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

fn bool_from_bits(bits: u64) -> bool {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return false;
    }
    if let Some(i) = to_i64(obj) {
        return i != 0;
    }
    // truthy fallback
    true
}

fn int_from_bits_default(_py: &PyToken<'_>, bits: u64, default: i64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return default;
    }
    if let Some(i) = to_i64(obj) {
        return i;
    }
    default
}

// ─── base64 core ────────────────────────────────────────────────────────────

fn b64_encode(input: &[u8], alphabet: &[u8; 64]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() { input[idx + 1] } else { 0 };
        let b2 = if idx + 2 < input.len() { input[idx + 2] } else { 0 };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(alphabet[((n >> 18) & 0x3f) as usize]);
        out.push(alphabet[((n >> 12) & 0x3f) as usize]);
        if idx + 1 < input.len() {
            out.push(alphabet[((n >> 6) & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        if idx + 2 < input.len() {
            out.push(alphabet[(n & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        idx += 3;
    }
    out
}

fn b64_decode_table(alphabet: &[u8; 64]) -> [Option<u8>; 256] {
    let mut table = [None; 256];
    for (i, &ch) in alphabet.iter().enumerate() {
        table[ch as usize] = Some(i as u8);
    }
    table
}

fn b64_decode(input: &[u8], alphabet: &[u8; 64], validate: bool) -> Result<Vec<u8>, &'static str> {
    let table = b64_decode_table(alphabet);

    let filtered: Vec<u8> = if validate {
        // In validate mode, reject any whitespace or non-base64 characters
        for &b in input {
            if b == b'\n' || b == b'\r' || b == b'\t' || b == b' ' {
                return Err("Invalid base64-encoded string: number of data characters (0) cannot be 1 more than a multiple of 4");
            }
            if table[b as usize].is_none() && b != b'=' {
                return Err("Invalid base64-encoded string: number of data characters (0) cannot be 1 more than a multiple of 4");
            }
        }
        input.to_vec()
    } else {
        // Non-validate mode: strip non-alphabet chars except = and whitespace
        input
            .iter()
            .copied()
            .filter(|&b| table[b as usize].is_some() || b == b'=')
            .collect()
    };

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    // Pad to multiple of 4
    let mut data = filtered;
    let remainder = data.len() % 4;
    if remainder != 0 {
        if validate {
            return Err("Incorrect padding");
        }
        for _ in 0..(4 - remainder) {
            data.push(b'=');
        }
    }

    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut idx = 0usize;
    while idx < data.len() {
        let c0 = data[idx];
        let c1 = data[idx + 1];
        let c2 = data[idx + 2];
        let c3 = data[idx + 3];

        let Some(v0) = table[c0 as usize] else {
            return Err("Non-base64 digit found");
        };
        let Some(v1) = table[c1 as usize] else {
            return Err("Non-base64 digit found");
        };
        let pad2 = c2 == b'=';
        let pad3 = c3 == b'=';
        let v2 = if pad2 {
            0
        } else if let Some(v) = table[c2 as usize] {
            v
        } else {
            return Err("Non-base64 digit found");
        };
        let v3 = if pad3 {
            0
        } else if let Some(v) = table[c3 as usize] {
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

// ─── base32 core ────────────────────────────────────────────────────────────

fn b32_encode(input: &[u8], alphabet: &[u8; 32]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let leftover = input.len() % 5;
    let mut padded = input.to_vec();
    if leftover != 0 {
        padded.extend(std::iter::repeat(0u8).take(5 - leftover));
    }
    let mut out = Vec::with_capacity(padded.len().div_ceil(5) * 8);
    for chunk in padded.chunks(5) {
        let val = ((chunk[0] as u64) << 32)
            | ((chunk[1] as u64) << 24)
            | ((chunk[2] as u64) << 16)
            | ((chunk[3] as u64) << 8)
            | (chunk[4] as u64);
        out.push(alphabet[((val >> 35) & 0x1F) as usize]);
        out.push(alphabet[((val >> 30) & 0x1F) as usize]);
        out.push(alphabet[((val >> 25) & 0x1F) as usize]);
        out.push(alphabet[((val >> 20) & 0x1F) as usize]);
        out.push(alphabet[((val >> 15) & 0x1F) as usize]);
        out.push(alphabet[((val >> 10) & 0x1F) as usize]);
        out.push(alphabet[((val >> 5) & 0x1F) as usize]);
        out.push(alphabet[(val & 0x1F) as usize]);
    }
    // Apply padding
    match leftover {
        1 => {
            let len = out.len();
            out[len - 6..].fill(b'=');
        }
        2 => {
            let len = out.len();
            out[len - 4..].fill(b'=');
        }
        3 => {
            let len = out.len();
            out[len - 3..].fill(b'=');
        }
        4 => {
            let len = out.len();
            out[len - 1] = b'=';
        }
        _ => {}
    }
    out
}

fn b32_decode_table(alphabet: &[u8; 32]) -> [Option<u8>; 256] {
    let mut table = [None; 256];
    for (i, &ch) in alphabet.iter().enumerate() {
        table[ch as usize] = Some(i as u8);
    }
    table
}

fn b32_decode(
    input: &[u8],
    alphabet: &[u8; 32],
    casefold: bool,
    map01: Option<u8>,
) -> Result<Vec<u8>, &'static str> {
    let mut data = input.to_vec();

    // Apply map01 transformation: 0 -> O, 1 -> map01_char
    if let Some(map_byte) = map01 {
        for b in &mut data {
            if *b == b'0' {
                *b = b'O';
            } else if *b == b'1' {
                *b = map_byte;
            }
        }
    }

    if casefold {
        data.make_ascii_uppercase();
    }

    if data.len() % 8 != 0 {
        return Err("Incorrect padding");
    }

    let table = b32_decode_table(alphabet);
    let stripped_len = data.iter().rposition(|&b| b != b'=').map_or(0, |p| p + 1);
    let padchars = data.len() - stripped_len;

    if padchars != 0 && padchars != 1 && padchars != 3 && padchars != 4 && padchars != 6 {
        return Err("Incorrect padding");
    }

    let stripped = &data[..stripped_len];

    let mut out = Vec::with_capacity(stripped.len() * 5 / 8 + 5);
    for chunk_start in (0..stripped.len()).step_by(8) {
        let chunk = &stripped[chunk_start..];
        let chunk_len = chunk.len().min(8);
        let mut acc: u64 = 0;
        for i in 0..chunk_len {
            let Some(val) = table[chunk[i] as usize] else {
                return Err("Non-base32 digit found");
            };
            acc = (acc << 5) | (val as u64);
        }
        // Pad remaining bits if chunk < 8
        for _ in chunk_len..8 {
            acc <<= 5;
        }
        out.push(((acc >> 32) & 0xFF) as u8);
        out.push(((acc >> 24) & 0xFF) as u8);
        out.push(((acc >> 16) & 0xFF) as u8);
        out.push(((acc >> 8) & 0xFF) as u8);
        out.push((acc & 0xFF) as u8);
    }

    // Trim output based on padding
    let trim = match padchars {
        1 => 1,
        3 => 2,
        4 => 3,
        6 => 4,
        _ => 0,
    };
    if trim > 0 && out.len() >= trim {
        out.truncate(out.len() - trim);
    }

    Ok(out)
}

// ─── base16 / hex ───────────────────────────────────────────────────────────

fn b16_encode(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(input.len() * 2);
    for &byte in input {
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
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

fn b16_decode(input: &[u8], casefold: bool) -> Result<Vec<u8>, &'static str> {
    let data: Vec<u8> = if casefold {
        input.iter().map(|b| b.to_ascii_uppercase()).collect()
    } else {
        input.to_vec()
    };

    if data.len() % 2 != 0 {
        return Err("Odd-length string");
    }

    let mut out = Vec::with_capacity(data.len() / 2);
    let mut idx = 0usize;
    while idx < data.len() {
        let Some(hi) = hex_nibble(data[idx]) else {
            return Err("Non-hexadecimal digit found");
        };
        let Some(lo) = hex_nibble(data[idx + 1]) else {
            return Err("Non-hexadecimal digit found");
        };
        out.push((hi << 4) | lo);
        idx += 2;
    }
    Ok(out)
}

// ─── Ascii85 (a85) ─────────────────────────────────────────────────────────

fn a85_encode_word(value: u32) -> [u8; 5] {
    let mut digits = [0u8; 5];
    let mut v = value;
    for i in (0..5).rev() {
        digits[i] = (v % 85) as u8 + 33;
        v /= 85;
    }
    digits
}

fn a85_encode(input: &[u8], foldspaces: bool, wrapcol: usize, pad: bool, adobe: bool) -> Vec<u8> {
    if input.is_empty() {
        if adobe {
            let mut out = Vec::with_capacity(4);
            out.extend_from_slice(A85_START);
            out.extend_from_slice(A85_END);
            return out;
        }
        return Vec::new();
    }

    let padding = (4 - (input.len() % 4)) % 4;
    let mut padded = input.to_vec();
    for _ in 0..padding {
        padded.push(0);
    }

    let mut encoded = Vec::with_capacity(padded.len() * 5 / 4 + 16);
    for chunk in padded.chunks(4) {
        let word =
            ((chunk[0] as u32) << 24) | ((chunk[1] as u32) << 16) | ((chunk[2] as u32) << 8) | (chunk[3] as u32);
        if word == 0 {
            encoded.push(b'z');
            continue;
        }
        if foldspaces && word == 0x20202020 {
            encoded.push(b'y');
            continue;
        }
        encoded.extend_from_slice(&a85_encode_word(word));
    }

    // Trim padding from last group if not pad mode
    if padding > 0 && !pad {
        let last_len = encoded.len();
        // Check if last was a 'z' fold
        if encoded.last() == Some(&b'z') {
            encoded.pop();
            encoded.extend_from_slice(&a85_encode_word(0));
        }
        let new_len = encoded.len();
        encoded.truncate(new_len - padding);
        let _ = last_len; // suppress warning
    }

    let mut result = if adobe {
        let mut r = Vec::with_capacity(encoded.len() + 4);
        r.extend_from_slice(A85_START);
        r.extend_from_slice(&encoded);
        r
    } else {
        encoded
    };

    if wrapcol > 0 {
        let effective_col = if adobe {
            wrapcol.max(2)
        } else {
            wrapcol.max(1)
        };
        let mut wrapped = Vec::with_capacity(result.len() + result.len() / effective_col + 4);
        let mut col = 0;
        for &b in &result {
            if col > 0 && col >= effective_col {
                wrapped.push(b'\n');
                col = 0;
            }
            wrapped.push(b);
            col += 1;
        }
        if adobe {
            // Check if ~> fits on current line
            if col + 2 > effective_col {
                wrapped.push(b'\n');
            }
            wrapped.extend_from_slice(A85_END);
        }
        return wrapped;
    }

    if adobe {
        result.extend_from_slice(A85_END);
    }
    result
}

fn a85_decode(input: &[u8], foldspaces: bool, adobe: bool) -> Result<Vec<u8>, String> {
    let data = if adobe {
        if !input.ends_with(A85_END) {
            return Err(format!(
                "Ascii85 encoded byte sequences must end with {:?}",
                std::str::from_utf8(A85_END).unwrap_or("~>")
            ));
        }
        let start = if input.starts_with(A85_START) { 2 } else { 0 };
        &input[start..input.len() - 2]
    } else {
        input
    };

    // Remove whitespace
    let cleaned: Vec<u8> = data
        .iter()
        .copied()
        .filter(|&b| b != b' ' && b != b'\t' && b != b'\n' && b != b'\r' && b != 0x0b)
        .collect();

    let mut decoded = Vec::with_capacity(cleaned.len() * 4 / 5 + 4);
    let mut curr: Vec<u8> = Vec::with_capacity(5);

    for &ch in &cleaned {
        if (33..=117).contains(&ch) {
            curr.push(ch);
            if curr.len() == 5 {
                let mut acc: u64 = 0;
                for &digit in &curr {
                    acc = acc * 85 + (digit as u64 - 33);
                }
                if acc > 0xFFFFFFFF {
                    return Err("Ascii85 overflow".to_string());
                }
                decoded.extend_from_slice(&(acc as u32).to_be_bytes());
                curr.clear();
            }
        } else if ch == b'z' {
            if !curr.is_empty() {
                return Err("z inside Ascii85 5-tuple".to_string());
            }
            decoded.extend_from_slice(&[0, 0, 0, 0]);
        } else if foldspaces && ch == b'y' {
            if !curr.is_empty() {
                return Err("y inside Ascii85 5-tuple".to_string());
            }
            decoded.extend_from_slice(&[0x20, 0x20, 0x20, 0x20]);
        } else {
            return Err(format!("Non-Ascii85 digit found: {}", ch as char));
        }
    }

    // Handle remaining partial group
    if !curr.is_empty() {
        let padding = 5 - curr.len();
        for _ in 0..padding {
            curr.push(b'u'); // 117 = max value char
        }
        let mut acc: u64 = 0;
        for &digit in &curr {
            acc = acc * 85 + (digit as u64 - 33);
        }
        if acc > 0xFFFFFFFF {
            return Err("Ascii85 overflow".to_string());
        }
        let bytes = (acc as u32).to_be_bytes();
        decoded.extend_from_slice(&bytes[..4 - padding]);
    }

    Ok(decoded)
}

// ─── base85 (RFC 1924) ─────────────────────────────────────────────────────

fn b85_decode_table() -> [Option<u8>; 256] {
    let mut table = [None; 256];
    for (i, &ch) in B85_ALPHABET.iter().enumerate() {
        table[ch as usize] = Some(i as u8);
    }
    table
}

fn b85_encode(input: &[u8], pad: bool) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }

    let padding = (4 - (input.len() % 4)) % 4;
    let mut padded = input.to_vec();
    for _ in 0..padding {
        padded.push(0);
    }

    let mut out = Vec::with_capacity(padded.len() * 5 / 4 + 1);
    for chunk in padded.chunks(4) {
        let word =
            ((chunk[0] as u32) << 24) | ((chunk[1] as u32) << 16) | ((chunk[2] as u32) << 8) | (chunk[3] as u32);
        let mut digits = [0u8; 5];
        let mut v = word;
        for i in (0..5).rev() {
            digits[i] = B85_ALPHABET[(v % 85) as usize];
            v /= 85;
        }
        out.extend_from_slice(&digits);
    }

    if padding > 0 && !pad {
        out.truncate(out.len() - padding);
    }
    out
}

fn b85_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let table = b85_decode_table();
    let padding = (5 - (input.len() % 5)) % 5;
    let mut data = input.to_vec();
    for _ in 0..padding {
        data.push(b'~'); // '~' maps to value 84 (max)
    }

    let mut out = Vec::with_capacity(data.len() * 4 / 5 + 4);
    for (chunk_idx, chunk) in data.chunks(5).enumerate() {
        let mut acc: u64 = 0;
        for (jdx, &ch) in chunk.iter().enumerate() {
            let Some(val) = table[ch as usize] else {
                let pos = chunk_idx * 5 + jdx;
                return Err(format!("bad base85 character at position {pos}"));
            };
            acc = acc * 85 + val as u64;
        }
        if acc > 0xFFFFFFFF {
            let pos = chunk_idx * 5;
            return Err(format!("base85 overflow in hunk starting at byte {pos}"));
        }
        out.extend_from_slice(&(acc as u32).to_be_bytes());
    }

    if padding > 0 {
        out.truncate(out.len() - padding);
    }
    Ok(out)
}

// ─── public intrinsics ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b64encode(data_bits: u64, altchars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "b64encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let mut alphabet = *B64_STD;
        let altchars_obj = obj_from_bits(altchars_bits);
        if !altchars_obj.is_none() {
            let alt = match bytes_like_arg(_py, altchars_bits, "b64encode") {
                Ok(v) => v,
                Err(bits) => return bits,
            };
            if alt.len() != 2 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "altchars must be a length-2 bytes-like object",
                );
            }
            // Replace + and / with custom chars
            for i in 0..64 {
                if alphabet[i] == b'+' {
                    alphabet[i] = alt[0];
                } else if alphabet[i] == b'/' {
                    alphabet[i] = alt[1];
                }
            }
        }
        let encoded = b64_encode(&raw, &alphabet);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b64decode(
    data_bits: u64,
    altchars_bits: u64,
    validate_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut raw = match bytes_like_arg(_py, data_bits, "b64decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let validate = bool_from_bits(validate_bits);

        let altchars_obj = obj_from_bits(altchars_bits);
        if !altchars_obj.is_none() {
            let alt = match bytes_like_arg(_py, altchars_bits, "b64decode") {
                Ok(v) => v,
                Err(bits) => return bits,
            };
            if alt.len() != 2 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "altchars must be a length-2 bytes-like object",
                );
            }
            // Translate altchars back to +/
            for b in &mut raw {
                if *b == alt[0] {
                    *b = b'+';
                } else if *b == alt[1] {
                    *b = b'/';
                }
            }
        }

        let decoded = match b64_decode(&raw, B64_STD, validate) {
            Ok(v) => v,
            Err(msg) => {
                return raise_exception::<_>(
                    _py,
                    "binascii.Error",
                    msg,
                );
            }
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_standard_b64encode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "standard_b64encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = b64_encode(&raw, B64_STD);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_standard_b64decode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "standard_b64decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let decoded = match b64_decode(&raw, B64_STD, false) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "binascii.Error", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_urlsafe_b64encode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "urlsafe_b64encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = b64_encode(&raw, B64_URLSAFE);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_urlsafe_b64decode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut raw = match bytes_like_arg(_py, data_bits, "urlsafe_b64decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        // Translate -_ back to +/
        for b in &mut raw {
            if *b == b'-' {
                *b = b'+';
            } else if *b == b'_' {
                *b = b'/';
            }
        }
        let decoded = match b64_decode(&raw, B64_STD, false) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "binascii.Error", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b32encode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "b32encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = b32_encode(&raw, B32_STD);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b32decode(
    data_bits: u64,
    casefold_bits: u64,
    map01_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b32decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let casefold = bool_from_bits(casefold_bits);
        let map01_obj = obj_from_bits(map01_bits);
        let map01 = if map01_obj.is_none() {
            None
        } else {
            // Extract the first byte of the map01 argument
            let m = match bytes_like_arg(_py, map01_bits, "b32decode") {
                Ok(v) => v,
                Err(bits) => return bits,
            };
            if m.len() != 1 {
                return raise_exception::<_>(_py, "ValueError", "map01 must be length 1");
            }
            Some(m[0])
        };
        let decoded = match b32_decode(&raw, B32_STD, casefold, map01) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "binascii.Error", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b32hexencode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "b32hexencode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = b32_encode(&raw, B32_HEX);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b32hexdecode(data_bits: u64, casefold_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b32hexdecode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let casefold = bool_from_bits(casefold_bits);
        let decoded = match b32_decode(&raw, B32_HEX, casefold, None) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "binascii.Error", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b16encode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "b16encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = b16_encode(&raw);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b16decode(data_bits: u64, casefold_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b16decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let casefold = bool_from_bits(casefold_bits);
        let decoded = match b16_decode(&raw, casefold) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_a85encode(
    data_bits: u64,
    foldspaces_bits: u64,
    wrapcol_bits: u64,
    pad_bits: u64,
    adobe_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "a85encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let foldspaces = bool_from_bits(foldspaces_bits);
        let wrapcol = int_from_bits_default(_py, wrapcol_bits, 0) as usize;
        let pad = bool_from_bits(pad_bits);
        let adobe = bool_from_bits(adobe_bits);
        let encoded = a85_encode(&raw, foldspaces, wrapcol, pad, adobe);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_a85decode(
    data_bits: u64,
    foldspaces_bits: u64,
    adobe_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "a85decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let foldspaces = bool_from_bits(foldspaces_bits);
        let adobe = bool_from_bits(adobe_bits);
        let decoded = match a85_decode(&raw, foldspaces, adobe) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b85encode(data_bits: u64, pad_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "b85encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let pad = bool_from_bits(pad_bits);
        let encoded = b85_encode(&raw, pad);
        alloc_bytes_result(_py, &encoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_b85decode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b85decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let decoded = match b85_decode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_encodebytes(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match encode_bytes_like_arg(_py, data_bits, "encodebytes") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        if raw.is_empty() {
            return alloc_bytes_result(_py, b"");
        }
        let mut result = Vec::with_capacity(raw.len() * 4 / 3 + raw.len() / MAXBINSIZE + 4);
        for chunk_start in (0..raw.len()).step_by(MAXBINSIZE) {
            let chunk_end = (chunk_start + MAXBINSIZE).min(raw.len());
            let chunk = &raw[chunk_start..chunk_end];
            let encoded = b64_encode(chunk, B64_STD);
            result.extend_from_slice(&encoded);
            result.push(b'\n');
        }
        alloc_bytes_result(_py, &result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_base64_decodebytes(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "decodebytes") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let decoded = match b64_decode(&raw, B64_STD, false) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "binascii.Error", msg),
        };
        alloc_bytes_result(_py, &decoded)
    })
}
