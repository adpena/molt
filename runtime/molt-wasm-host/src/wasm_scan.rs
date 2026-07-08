use super::*;

fn read_varuint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*offset)?;
        *offset += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 70 {
            return None;
        }
    }
}

fn read_varint32(data: &[u8], offset: &mut usize) -> Option<i32> {
    let mut result = 0i64;
    let mut shift = 0u32;
    let mut byte;
    loop {
        byte = *data.get(*offset)?;
        *offset += 1;
        result |= i64::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
        if shift >= 35 {
            return None;
        }
    }
    if shift < 32 && (byte & 0x40) != 0 {
        result |= !0i64 << shift;
    }
    i32::try_from(result).ok()
}

fn read_wasm_name<'a>(data: &'a [u8], offset: &mut usize) -> Option<&'a str> {
    let len = usize::try_from(read_varuint(data, offset)?).ok()?;
    let end = offset.checked_add(len)?;
    let bytes = data.get(*offset..end)?;
    *offset = end;
    std::str::from_utf8(bytes).ok()
}

fn skip_wasm_limits(data: &[u8], offset: &mut usize) -> Option<()> {
    let flags = read_varuint(data, offset)?;
    read_varuint(data, offset)?;
    if flags & 0x01 != 0 {
        read_varuint(data, offset)?;
    }
    Some(())
}

fn skip_wasm_import_desc(data: &[u8], offset: &mut usize, kind: u8) -> Option<()> {
    match kind {
        0 => {
            read_varuint(data, offset)?;
        }
        1 => {
            *offset = offset.checked_add(1)?;
            skip_wasm_limits(data, offset)?;
        }
        2 => {
            skip_wasm_limits(data, offset)?;
        }
        3 => {
            *offset = offset.checked_add(2)?;
            data.get(*offset - 1)?;
        }
        4 => {
            *offset = offset.checked_add(1)?;
            read_varuint(data, offset)?;
        }
        _ => return None,
    }
    Some(())
}

fn read_wasm_const_expr_i32(data: &[u8], offset: &mut usize) -> Option<Option<i32>> {
    let opcode = *data.get(*offset)?;
    *offset += 1;
    let value = match opcode {
        0x41 => Some(read_varint32(data, offset)?),
        0x23 | 0xd2 => {
            read_varuint(data, offset)?;
            None
        }
        0xd0 => {
            *offset = offset.checked_add(1)?;
            data.get(*offset - 1)?;
            None
        }
        _ => return None,
    };
    if *data.get(*offset)? != 0x0b {
        return None;
    }
    *offset += 1;
    Some(value)
}

fn skip_wasm_element_items(
    data: &[u8],
    offset: &mut usize,
    count: u64,
    uses_expressions: bool,
) -> Option<()> {
    for _ in 0..count {
        if uses_expressions {
            read_wasm_const_expr_i32(data, offset)?;
        } else {
            read_varuint(data, offset)?;
        }
    }
    Some(())
}

fn parse_table_ref_export_base(name: &str) -> Option<u64> {
    let raw = name.strip_prefix("__molt_table_ref_")?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = raw.parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

pub(super) fn detect_wasm_table_base(path: &Path) -> Result<Option<u64>> {
    let data = fs::read(path).with_context(|| format!("read wasm table-base probe {path:?}"))?;
    if data.len() < 8 || &data[..4] != b"\0asm" {
        return Ok(None);
    }
    let mut offset = 8usize;
    let mut import_func_count = 0u64;
    let mut table_init_func_index: Option<u64> = None;
    let mut code_bodies: Vec<(usize, usize)> = Vec::new();
    let mut active_table_bases: Vec<u64> = Vec::new();
    let mut exported_base: Option<u64> = None;
    while offset < data.len() {
        let section_id = data[offset];
        offset += 1;
        let Some(section_size) = read_varuint(&data, &mut offset) else {
            return Ok(exported_base);
        };
        let Ok(section_size) = usize::try_from(section_size) else {
            return Ok(exported_base);
        };
        let Some(section_end) = offset.checked_add(section_size) else {
            return Ok(exported_base);
        };
        if section_end > data.len() {
            return Ok(exported_base);
        }
        match section_id {
            2 => {
                let Some(count) = read_varuint(&data, &mut offset) else {
                    return Ok(exported_base);
                };
                for _ in 0..count {
                    if read_wasm_name(&data, &mut offset).is_none()
                        || read_wasm_name(&data, &mut offset).is_none()
                    {
                        return Ok(exported_base);
                    }
                    let Some(kind) = data.get(offset).copied() else {
                        return Ok(exported_base);
                    };
                    offset += 1;
                    if kind == 0 {
                        import_func_count += 1;
                    }
                    if skip_wasm_import_desc(&data, &mut offset, kind).is_none() {
                        return Ok(exported_base);
                    }
                }
            }
            7 => {
                let Some(count) = read_varuint(&data, &mut offset) else {
                    return Ok(exported_base);
                };
                for _ in 0..count {
                    let Some(name) = read_wasm_name(&data, &mut offset) else {
                        return Ok(exported_base);
                    };
                    let Some(kind) = data.get(offset).copied() else {
                        return Ok(exported_base);
                    };
                    offset += 1;
                    let Some(index) = read_varuint(&data, &mut offset) else {
                        return Ok(exported_base);
                    };
                    if kind == 0 && name == "molt_table_init" {
                        table_init_func_index = Some(index);
                    }
                    if let Some(base) = parse_table_ref_export_base(name)
                        && exported_base.map(|current| base < current).unwrap_or(true)
                    {
                        exported_base = Some(base);
                    }
                }
            }
            9 => {
                let Some(count) = read_varuint(&data, &mut offset) else {
                    return Ok(exported_base);
                };
                for _ in 0..count {
                    let Some(flags) = read_varuint(&data, &mut offset) else {
                        return Ok(exported_base);
                    };
                    let uses_expressions = flags & 0x04 != 0;
                    let is_active = matches!(flags, 0 | 2 | 4 | 6);
                    if matches!(flags, 2 | 6) && read_varuint(&data, &mut offset).is_none() {
                        return Ok(exported_base);
                    }
                    if is_active {
                        let Some(table_base) = read_wasm_const_expr_i32(&data, &mut offset) else {
                            return Ok(exported_base);
                        };
                        if matches!(flags, 2 | 3 | 5 | 6 | 7) {
                            offset = offset.saturating_add(1);
                        }
                        if let Some(base) = table_base.and_then(|base| u64::try_from(base).ok())
                            && base > 0
                            && exported_base.map(|export| base >= export).unwrap_or(true)
                        {
                            active_table_bases.push(base);
                        }
                    } else if matches!(flags, 1 | 3 | 5 | 7) {
                        offset = offset.saturating_add(1);
                    } else {
                        return Ok(exported_base);
                    }
                    let Some(item_count) = read_varuint(&data, &mut offset) else {
                        return Ok(exported_base);
                    };
                    if skip_wasm_element_items(&data, &mut offset, item_count, uses_expressions)
                        .is_none()
                    {
                        return Ok(exported_base);
                    }
                }
            }
            10 => {
                let Some(count) = read_varuint(&data, &mut offset) else {
                    return Ok(exported_base);
                };
                for _ in 0..count {
                    let Some(body_size) = read_varuint(&data, &mut offset) else {
                        return Ok(exported_base);
                    };
                    let Ok(body_size) = usize::try_from(body_size) else {
                        return Ok(exported_base);
                    };
                    let body_start = offset;
                    let Some(body_end) = body_start.checked_add(body_size) else {
                        return Ok(exported_base);
                    };
                    if body_end > section_end {
                        return Ok(exported_base);
                    }
                    code_bodies.push((body_start, body_end));
                    offset = body_end;
                }
            }
            _ => offset = section_end,
        }
        if offset != section_end && section_id != 10 {
            offset = section_end;
        }
    }
    if let Some(table_init_index) = table_init_func_index
        && let Some(defined_index) = table_init_index.checked_sub(import_func_count)
        && let Ok(defined_index) = usize::try_from(defined_index)
        && let Some((body_start, body_end)) = code_bodies.get(defined_index).copied()
    {
        let mut pos = body_start;
        if let Some(local_decl_count) = read_varuint(&data, &mut pos) {
            for _ in 0..local_decl_count {
                if read_varuint(&data, &mut pos).is_none() || pos >= body_end {
                    break;
                }
                pos += 1;
            }
            if pos < body_end && data.get(pos) == Some(&0x41) {
                pos += 1;
                if let Some(base) = read_varint32(&data, &mut pos)
                    && let Ok(base) = u64::try_from(base)
                    && base > 0
                {
                    if let Some(exported) = exported_base
                        && base < exported
                    {
                        return Ok(Some(exported));
                    }
                    return Ok(Some(base));
                }
            }
        }
    }
    if !active_table_bases.is_empty() {
        return Ok(active_table_bases
            .iter()
            .copied()
            .filter(|base| *base > 1)
            .min()
            .or_else(|| active_table_bases.iter().copied().min()));
    }
    Ok(exported_base)
}
