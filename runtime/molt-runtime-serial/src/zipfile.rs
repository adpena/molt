#![allow(dead_code, unused_imports)]

use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::collections::HashSet;

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_crc32(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };
        let Some(bytes) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };

        let mut crc = 0xFFFF_FFFFu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                if (crc & 1) != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^= 0xFFFF_FFFF;
        MoltObject::from_int(i64::from(crc)).bits()
    })
}

fn imghdr_detect_kind(header: &[u8]) -> Option<&'static str> {
    if header.len() >= 10 && (header[6..10] == *b"JFIF" || header[6..10] == *b"Exif")
        || header.starts_with(b"\xFF\xD8\xFF\xDB")
    {
        return Some("jpeg");
    }
    if header.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Some("png");
    }
    if header.len() >= 6 && (header[..6] == *b"GIF87a" || header[..6] == *b"GIF89a") {
        return Some("gif");
    }
    if header.len() >= 2 && (header[..2] == *b"MM" || header[..2] == *b"II") {
        return Some("tiff");
    }
    if header.starts_with(b"\x01\xDA") {
        return Some("rgb");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'1' | b'4')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pbm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'2' | b'5')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pgm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'3' | b'6')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("ppm");
    }
    if header.starts_with(b"\x59\xA6\x6A\x95") {
        return Some("rast");
    }
    if header.starts_with(b"#define ") {
        return Some("xbm");
    }
    if header.starts_with(b"BM") {
        return Some("bmp");
    }
    if header.starts_with(b"RIFF") && header.len() >= 12 && header[8..12] == *b"WEBP" {
        return Some("webp");
    }
    if header.starts_with(b"\x76\x2f\x31\x01") {
        return Some("exr");
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_detect(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

const ZIPFILE_CENTRAL_SIG: [u8; 4] = *b"PK\x01\x02";
const ZIPFILE_EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const ZIPFILE_ZIP64_EOCD_SIG: [u8; 4] = *b"PK\x06\x06";
const ZIPFILE_ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
const ZIPFILE_ZIP64_LIMIT: u64 = 0xFFFF_FFFF;
const ZIPFILE_ZIP64_COUNT_LIMIT: u16 = 0xFFFF;
const ZIPFILE_ZIP64_EXTRA_ID: u16 = 0x0001;

fn zipfile_read_u16_le(data: &[u8], offset: usize, err: &'static str) -> Result<u16, &'static str> {
    let end = offset.checked_add(2).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn zipfile_read_u32_le(data: &[u8], offset: usize, err: &'static str) -> Result<u32, &'static str> {
    let end = offset.checked_add(4).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn zipfile_read_u64_le(data: &[u8], offset: usize, err: &'static str) -> Result<u64, &'static str> {
    let end = offset.checked_add(8).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn zipfile_find_eocd_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 65_535);
    data[start..]
        .windows(4)
        .rposition(|window| window == ZIPFILE_EOCD_SIG)
        .map(|idx| start + idx)
}

fn zipfile_read_zip64_eocd(
    data: &[u8],
    eocd_offset: usize,
) -> Result<(usize, usize), &'static str> {
    let locator_offset = eocd_offset.checked_sub(20).ok_or("zip64 locator missing")?;
    let locator_sig = data
        .get(locator_offset..locator_offset + 4)
        .ok_or("zip64 locator missing")?;
    if locator_sig != ZIPFILE_ZIP64_LOCATOR_SIG {
        return Err("zip64 locator missing");
    }
    let zip64_eocd_offset = zipfile_read_u64_le(data, locator_offset + 8, "zip64 locator missing")?;
    let zip64_eocd_offset =
        usize::try_from(zip64_eocd_offset).map_err(|_| "zip64 eocd offset overflow")?;
    let eocd_sig = data
        .get(zip64_eocd_offset..zip64_eocd_offset + 4)
        .ok_or("zip64 eocd missing")?;
    if eocd_sig != ZIPFILE_ZIP64_EOCD_SIG {
        return Err("zip64 eocd missing");
    }
    let cd_size = zipfile_read_u64_le(data, zip64_eocd_offset + 40, "zip64 eocd missing")?;
    let cd_offset = zipfile_read_u64_le(data, zip64_eocd_offset + 48, "zip64 eocd missing")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "zip64 central directory too large")?;
    let cd_offset =
        usize::try_from(cd_offset).map_err(|_| "zip64 central directory offset too large")?;
    Ok((cd_offset, cd_size))
}

fn zipfile_parse_zip64_extra(
    extra: &[u8],
    mut comp_size: u64,
    mut uncomp_size: u64,
    mut local_offset: u64,
) -> Result<(u64, u64, u64), &'static str> {
    let mut pos = 0usize;
    while pos + 4 <= extra.len() {
        let header_id = zipfile_read_u16_le(extra, pos, "zip64 extra missing")?;
        let data_size = usize::from(zipfile_read_u16_le(extra, pos + 2, "zip64 extra missing")?);
        pos += 4;
        let Some(data_end) = pos.checked_add(data_size) else {
            return Err("zip64 extra missing");
        };
        if data_end > extra.len() {
            break;
        }
        if header_id == ZIPFILE_ZIP64_EXTRA_ID {
            let mut cursor = pos;
            if uncomp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing size");
                }
                uncomp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing size")?;
                cursor += 8;
            }
            if comp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing comp size");
                }
                comp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing comp size")?;
                cursor += 8;
            }
            if local_offset == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing offset");
                }
                local_offset = zipfile_read_u64_le(extra, cursor, "zip64 extra missing offset")?;
            }
            return Ok((comp_size, uncomp_size, local_offset));
        }
        pos = data_end;
    }
    Err("zip64 extra missing")
}

fn zipfile_parse_central_directory_impl(
    data: &[u8],
) -> Result<Vec<(String, [u64; 5])>, &'static str> {
    if data.len() < 22 {
        return Err("file is not a zip file");
    }
    let Some(eocd_offset) = zipfile_find_eocd_offset(data) else {
        return Err("end of central directory not found");
    };

    let mut cd_size = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 12,
        "end of central directory not found",
    )?);
    let mut cd_offset = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 16,
        "end of central directory not found",
    )?);
    let total_entries =
        zipfile_read_u16_le(data, eocd_offset + 10, "end of central directory not found")?;
    if total_entries == ZIPFILE_ZIP64_COUNT_LIMIT
        || cd_size == ZIPFILE_ZIP64_LIMIT
        || cd_offset == ZIPFILE_ZIP64_LIMIT
    {
        let (zip64_offset, zip64_size) = zipfile_read_zip64_eocd(data, eocd_offset)?;
        cd_offset = zip64_offset as u64;
        cd_size = zip64_size as u64;
    }

    let pos_start = usize::try_from(cd_offset).map_err(|_| "central directory offset overflow")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "central directory size overflow")?;
    let Some(end) = pos_start.checked_add(cd_size) else {
        return Err("central directory overflow");
    };
    if end > data.len() {
        return Err("end of central directory not found");
    }

    let mut out: Vec<(String, [u64; 5])> = Vec::new();
    let mut pos = pos_start;
    while pos + 46 <= end {
        if data[pos..pos + 4] != ZIPFILE_CENTRAL_SIG {
            break;
        }
        let comp_method = u64::from(zipfile_read_u16_le(
            data,
            pos + 10,
            "invalid central directory entry",
        )?);
        let mut comp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 20,
            "invalid central directory entry",
        )?);
        let mut uncomp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 24,
            "invalid central directory entry",
        )?);
        let name_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 28,
            "invalid central directory entry",
        )?);
        let extra_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 30,
            "invalid central directory entry",
        )?);
        let comment_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 32,
            "invalid central directory entry",
        )?);
        let mut local_offset = u64::from(zipfile_read_u32_le(
            data,
            pos + 42,
            "invalid central directory entry",
        )?);

        let name_start = pos + 46;
        let Some(name_end) = name_start.checked_add(name_len) else {
            return Err("invalid central directory entry");
        };
        let Some(extra_end) = name_end.checked_add(extra_len) else {
            return Err("invalid central directory entry");
        };
        let Some(record_end) = extra_end.checked_add(comment_len) else {
            return Err("invalid central directory entry");
        };
        if record_end > end || record_end > data.len() {
            return Err("invalid central directory entry");
        }

        let name_bytes = &data[name_start..name_end];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(value) => value.to_string(),
            Err(_) => String::from_utf8_lossy(name_bytes).into_owned(),
        };

        if comp_size == ZIPFILE_ZIP64_LIMIT
            || uncomp_size == ZIPFILE_ZIP64_LIMIT
            || local_offset == ZIPFILE_ZIP64_LIMIT
        {
            let extra = &data[name_end..extra_end];
            let (parsed_comp, parsed_uncomp, parsed_offset) =
                zipfile_parse_zip64_extra(extra, comp_size, uncomp_size, local_offset)?;
            comp_size = parsed_comp;
            uncomp_size = parsed_uncomp;
            local_offset = parsed_offset;
        }

        out.push((
            name,
            [
                local_offset,
                comp_size,
                comp_method,
                name_len as u64,
                uncomp_size,
            ],
        ));
        pos = record_end;
    }
    Ok(out)
}

fn zipfile_build_zip64_extra_impl(size: u64, comp_size: u64, offset: Option<u64>) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::with_capacity(if offset.is_some() { 24 } else { 16 });
    data.extend_from_slice(size.to_le_bytes().as_slice());
    data.extend_from_slice(comp_size.to_le_bytes().as_slice());
    if let Some(offset) = offset {
        data.extend_from_slice(offset.to_le_bytes().as_slice());
    }
    let mut out: Vec<u8> = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(ZIPFILE_ZIP64_EXTRA_ID.to_le_bytes().as_slice());
    out.extend_from_slice((data.len() as u16).to_le_bytes().as_slice());
    out.extend_from_slice(data.as_slice());
    out
}

fn zipfile_trim_trailing_slashes(path: &str) -> &str {
    let mut end = path.len();
    while end > 0 && path.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

fn zipfile_ancestry(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = zipfile_trim_trailing_slashes(path).to_string();
    while !zipfile_trim_trailing_slashes(current.as_str()).is_empty() {
        out.push(current.clone());
        if let Some(idx) = current.rfind('/') {
            current.truncate(idx);
            while current.ends_with('/') {
                current.pop();
            }
        } else {
            current.clear();
        }
    }
    out
}

fn zipfile_parent_of(path: &str) -> &str {
    let trimmed = zipfile_trim_trailing_slashes(path);
    if let Some(idx) = trimmed.rfind('/') {
        &trimmed[..idx]
    } else {
        ""
    }
}

fn zipfile_escape_regex_char(ch: char, out: &mut String) {
    if matches!(
        ch,
        '.' | '^' | '$' | '*' | '+' | '?' | '{' | '}' | '[' | ']' | '\\' | '|' | '(' | ')'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn zipfile_escape_character_class(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(ch, '\\' | ']' | '^' | '-') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn zipfile_separate_pattern(pattern: &str) -> Vec<(bool, String)> {
    let bytes = pattern.as_bytes();
    let mut idx = 0usize;
    let mut out: Vec<(bool, String)> = Vec::new();
    while idx < bytes.len() {
        if bytes[idx] != b'[' {
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'[' {
                idx += 1;
            }
            out.push((false, pattern[start..idx].to_string()));
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < bytes.len() && bytes[idx] != b']' {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b']' {
            idx += 1;
            out.push((true, pattern[start..idx].to_string()));
            continue;
        }
        out.push((false, pattern[start..].to_string()));
        break;
    }
    out
}

fn zipfile_translate_chunk(chunk: &str, star_pattern: &str, qmark_pattern: &str) -> String {
    let chars: Vec<char> = chunk.chars().collect();
    let mut idx = 0usize;
    let mut out = String::new();
    while idx < chars.len() {
        match chars[idx] {
            '*' => {
                if idx + 1 < chars.len() && chars[idx + 1] == '*' {
                    out.push_str(".*");
                    idx += 2;
                } else {
                    out.push_str(star_pattern);
                    idx += 1;
                }
            }
            '?' => {
                out.push_str(qmark_pattern);
                idx += 1;
            }
            ch => {
                zipfile_escape_regex_char(ch, &mut out);
                idx += 1;
            }
        }
    }
    out
}

fn zipfile_star_not_empty(pattern: &str, seps: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if !segment.is_empty() {
                if segment == "*" {
                    out.push_str("?*");
                } else {
                    out.push_str(segment.as_str());
                }
                segment.clear();
            }
            out.push(ch);
        } else {
            segment.push(ch);
        }
    }
    if !segment.is_empty() {
        if segment == "*" {
            out.push_str("?*");
        } else {
            out.push_str(segment.as_str());
        }
    }
    out
}

fn zipfile_contains_invalid_rglob_segment(pattern: &str, seps: &str) -> bool {
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if segment.contains("**") && segment != "**" {
                return true;
            }
            segment.clear();
        } else {
            segment.push(ch);
        }
    }
    segment.contains("**") && segment != "**"
}

fn zipfile_translate_glob_impl(
    pattern: &str,
    seps: &str,
    py313_plus: bool,
) -> Result<String, &'static str> {
    let mut effective_pattern = pattern.to_string();
    let mut star_pattern = "[^/]*".to_string();
    let mut qmark_pattern = ".".to_string();
    if py313_plus {
        if zipfile_contains_invalid_rglob_segment(pattern, seps) {
            return Err("** must appear alone in a path segment");
        }
        effective_pattern = zipfile_star_not_empty(pattern, seps);
        let escaped = zipfile_escape_character_class(seps);
        star_pattern = format!("[^{escaped}]*");
        qmark_pattern = "[^/]".to_string();
    }

    let mut core = String::new();
    for (is_set, chunk) in zipfile_separate_pattern(effective_pattern.as_str()) {
        if is_set {
            core.push_str(chunk.as_str());
            continue;
        }
        core.push_str(
            zipfile_translate_chunk(
                chunk.as_str(),
                star_pattern.as_str(),
                qmark_pattern.as_str(),
            )
            .as_str(),
        );
    }

    let with_dirs = format!("{core}[/]?");
    if py313_plus {
        Ok(format!("(?s:{with_dirs})\\Z"))
    } else {
        Ok(with_dirs)
    }
}

fn zipfile_normalize_member_path_impl(member: &str) -> Option<String> {
    let replaced = member.replace('\\', "/");
    let mut stack: Vec<String> = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if let Some(last) = stack.last()
                && last != ".."
            {
                stack.pop();
                continue;
            }
            stack.push("..".to_string());
            continue;
        }
        stack.push(segment.to_string());
    }
    let normalized = stack.join("/");
    if normalized.is_empty() || normalized == "." {
        return None;
    }
    if normalized == ".." || normalized.starts_with("../") {
        return None;
    }
    Some(normalized)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_parse_central_directory(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let Some(data) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let entries = match zipfile_parse_central_directory_impl(data) {
            Ok(value) => value,
            Err(message) => return raise_exception::<_>(_py, "ValueError", message),
        };

        let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        for (name, fields) in entries {
            let Some(name_bits) = alloc_string_bits(_py, name.as_str()) else {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };

            let mut item_bits: [u64; 5] = [0; 5];
            for (idx, field) in fields.iter().enumerate() {
                let Ok(value) = i64::try_from(*field) else {
                    dec_ref_bits(_py, name_bits);
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "zipfile central directory value overflow",
                    );
                };
                item_bits[idx] = MoltObject::from_int(value).bits();
            }
            let tuple_ptr = alloc_tuple(_py, &item_bits);
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, name_bits);
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            pairs.push(name_bits);
            pairs.push(tuple_bits);
            owned_bits.push(name_bits);
            owned_bits.push(tuple_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let out = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_build_zip64_extra(
    size_bits: u64,
    comp_size_bits: u64,
    offset_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(size) = to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 size must be int");
        };
        if size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 size must be >= 0");
        }
        let Some(comp_size) = to_i64(obj_from_bits(comp_size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 comp_size must be int");
        };
        if comp_size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 comp_size must be >= 0");
        }
        let offset = if obj_from_bits(offset_bits).is_none() {
            None
        } else {
            let Some(value) = to_i64(obj_from_bits(offset_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "zipfile zip64 offset must be int");
            };
            if value < 0 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "zipfile zip64 offset must be >= 0",
                );
            }
            Some(value as u64)
        };

        let out = zipfile_build_zip64_extra_impl(size as u64, comp_size as u64, offset);
        let ptr = alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_implied_dirs(names_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.iter().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();

        for name in &names {
            let ancestry = zipfile_ancestry(name.as_str());
            for parent in ancestry.into_iter().skip(1) {
                let candidate = format!("{parent}/");
                if names_set.contains(candidate.as_str()) {
                    continue;
                }
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
            }
        }
        alloc_string_list(_py, out.as_slice())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_resolve_dir(name_bits: u64, names_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path name must be str");
        };
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.into_iter().collect();
        let mut resolved = name.clone();
        let dirname = format!("{name}/");
        if !names_set.contains(name.as_str()) && names_set.contains(dirname.as_str()) {
            resolved = dirname;
        }
        let ptr = alloc_string(_py, resolved.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_is_child(path_at_bits: u64, parent_at_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(path_at) = string_obj_to_owned(obj_from_bits(path_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path candidate must be str");
        };
        let Some(parent_at) = string_obj_to_owned(obj_from_bits(parent_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path parent must be str");
        };

        let candidate_parent = zipfile_parent_of(path_at.as_str());
        let parent_norm = zipfile_trim_trailing_slashes(parent_at.as_str());
        if candidate_parent == parent_norm {
            MoltObject::from_bool(true).bits()
        } else {
            MoltObject::from_bool(false).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_translate_glob(
    pattern_bits: u64,
    seps_bits: u64,
    py313_plus_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob pattern must be str");
        };
        let Some(seps) = string_obj_to_owned(obj_from_bits(seps_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob separators must be str");
        };
        let py313_plus = is_truthy(_py, obj_from_bits(py313_plus_bits));

        let translated =
            match zipfile_translate_glob_impl(pattern.as_str(), seps.as_str(), py313_plus) {
                Ok(value) => value,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
        let ptr = alloc_string(_py, translated.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_normalize_member_path(member_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(member) = string_obj_to_owned(obj_from_bits(member_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile member path must be str");
        };
        let Some(normalized) = zipfile_normalize_member_path_impl(member.as_str()) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, normalized.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_what(data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_test(kind_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr kind must be str");
        };
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let matches = imghdr_detect_kind(header)
            .map(|detected| detected == kind.as_str())
            .unwrap_or(false);
        MoltObject::from_bool(matches).bits()
    })
}

#[cfg(test)]
mod zipfile_path_lowering_tests {
    use super::{
        zipfile_ancestry, zipfile_build_zip64_extra_impl, zipfile_normalize_member_path_impl,
        zipfile_parse_central_directory_impl, zipfile_translate_glob_impl,
    };

    #[test]
    fn zipfile_ancestry_preserves_posix_structure() {
        assert_eq!(
            zipfile_ancestry("//b//d///f//"),
            vec![
                String::from("//b//d///f"),
                String::from("//b//d"),
                String::from("//b"),
            ]
        );
    }

    #[test]
    fn zipfile_translate_glob_legacy_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("*.txt", "/", false).expect("legacy translation");
        assert_eq!(translated, String::from("[^/]*\\.txt[/]?"));
    }

    #[test]
    fn zipfile_translate_glob_modern_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("**/*", "/", true).expect("modern translation");
        assert_eq!(translated, String::from("(?s:.*/[^/][^/]*[/]?)\\Z"));
    }

    #[test]
    fn zipfile_translate_glob_rejects_invalid_rglob_segment() {
        let err = zipfile_translate_glob_impl("**foo", "/", true).expect_err("invalid segment");
        assert_eq!(err, "** must appear alone in a path segment");
    }

    #[test]
    fn zipfile_normalize_member_path_blocks_traversal() {
        assert_eq!(
            zipfile_normalize_member_path_impl("safe/../leaf.txt"),
            Some(String::from("leaf.txt"))
        );
        assert_eq!(zipfile_normalize_member_path_impl("../escape.txt"), None);
        assert_eq!(zipfile_normalize_member_path_impl("./"), None);
    }

    #[test]
    fn zipfile_parse_central_directory_roundtrip_shape() {
        let name = b"a.txt";
        let payload = b"hello";
        let name_len = name.len() as u16;
        let payload_len = payload.len() as u32;

        let mut archive: Vec<u8> = Vec::new();
        archive.extend_from_slice(b"PK\x03\x04");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(name);
        archive.extend_from_slice(payload);

        let cd_offset = archive.len() as u32;
        archive.extend_from_slice(b"PK\x01\x02");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(name);

        let cd_size = (46 + name.len()) as u32;
        archive.extend_from_slice(b"PK\x05\x06");
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_size.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_offset.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());

        let parsed = zipfile_parse_central_directory_impl(archive.as_slice())
            .expect("central directory parse should succeed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, String::from("a.txt"));
        assert_eq!(
            parsed[0].1,
            [
                0,
                payload.len() as u64,
                0,
                name.len() as u64,
                payload.len() as u64
            ]
        );
    }

    #[test]
    fn zipfile_build_zip64_extra_shape() {
        let out = zipfile_build_zip64_extra_impl(7, 11, Some(13));
        assert_eq!(
            out,
            vec![
                0x01, 0x00, 24, 0, 7, 0, 0, 0, 0, 0, 0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 0,
                0, 0, 0,
            ]
        );
    }
}
