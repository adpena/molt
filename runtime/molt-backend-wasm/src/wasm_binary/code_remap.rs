use wasmparser::{BinaryReader, FunctionBody, Operator};

use super::leb::{encode_u32_leb128, read_u32_leb128};

struct IndexPatch {
    operand_start: usize,
    operand_end: usize,
    new_index: u32,
}

pub(super) fn remap_code_section(
    section_content: &[u8],
    remap: &dyn Fn(u32) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(section_content.len());
    let (count, mut offset) =
        read_u32_leb128(section_content, 0).ok_or("code section missing function count")?;
    encode_u32_leb128(count, &mut out);

    for body_index in 0..count {
        let (body_size, body_start) = read_u32_leb128(section_content, offset)
            .ok_or_else(|| format!("code section body {body_index} missing size"))?;
        let body_end = body_start
            .checked_add(body_size as usize)
            .filter(|end| *end <= section_content.len())
            .ok_or_else(|| format!("code section body {body_index} size overflows section"))?;
        let body = &section_content[body_start..body_end];
        let new_body = remap_function_body(body, remap)
            .map_err(|err| format!("code section body {body_index}: {err}"))?;

        encode_u32_leb128(new_body.len() as u32, &mut out);
        out.extend_from_slice(&new_body);
        offset = body_end;
    }

    if offset != section_content.len() {
        return Err("code section contains trailing bytes after declared bodies".to_string());
    }

    Ok(out)
}

fn remap_function_body(
    body: &[u8],
    remap: &dyn Fn(u32) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    let patches = collect_function_index_patches(body, remap)?;
    if patches.is_empty() {
        return Ok(body.to_vec());
    }

    let mut out = Vec::with_capacity(body.len());
    let mut cursor = 0usize;
    for patch in patches {
        if patch.operand_start < cursor || patch.operand_end < patch.operand_start {
            return Err("overlapping function-index operand patches".to_string());
        }
        out.extend_from_slice(&body[cursor..patch.operand_start]);
        encode_u32_leb128_width(
            patch.new_index,
            patch.operand_end - patch.operand_start,
            &mut out,
        )?;
        cursor = patch.operand_end;
    }
    out.extend_from_slice(&body[cursor..]);
    Ok(out)
}

fn encode_u32_leb128_width(value: u32, width: usize, out: &mut Vec<u8>) -> Result<(), String> {
    if width == 0 {
        return Err("function-index operand width cannot be zero".to_string());
    }
    let mut remaining = value;
    for byte_index in 0..width {
        let mut byte = (remaining & 0x7f) as u8;
        remaining >>= 7;
        if byte_index + 1 < width {
            byte |= 0x80;
        } else if remaining != 0 {
            return Err(format!(
                "function index {value} does not fit in original {width}-byte LEB128 operand"
            ));
        }
        out.push(byte);
    }
    Ok(())
}

fn collect_function_index_patches(
    body: &[u8],
    remap: &dyn Fn(u32) -> Result<u32, String>,
) -> Result<Vec<IndexPatch>, String> {
    let function_body = FunctionBody::new(BinaryReader::new(body, 0));
    let op_reader = function_body
        .get_operators_reader()
        .map_err(|err| format!("failed to parse function operators: {err}"))?;
    let mut patches = Vec::new();

    for op_result in op_reader.into_iter_with_offsets() {
        let (op, op_offset) =
            op_result.map_err(|err| format!("failed to read function operator: {err}"))?;
        let old_index = match op {
            Operator::Call { function_index }
            | Operator::ReturnCall { function_index }
            | Operator::RefFunc { function_index } => function_index,
            _ => continue,
        };
        let operand_start = op_offset
            .checked_add(1)
            .ok_or("function-index operand offset overflow")?;
        let (_, operand_end) = read_u32_leb128(body, operand_start)
            .ok_or("function-index operand is not a valid u32 LEB128")?;
        patches.push(IndexPatch {
            operand_start,
            operand_end,
            new_index: remap(old_index)?,
        });
    }

    Ok(patches)
}

#[cfg(test)]
mod tests {
    use super::remap_function_body;

    #[test]
    fn parser_backed_body_remap_preserves_non_index_operands() {
        let body = [
            0x00, // local decl count
            0x41, 0x10, // i32.const 16; operand byte equals call opcode
            0x1a, // drop
            0x10, 0x02, // call 2
            0xd2, 0x03, // ref.func 3
            0x0b, // end
        ];
        let remapped = remap_function_body(&body, &|idx| Ok(idx + 10)).unwrap();
        assert_eq!(
            remapped,
            vec![0x00, 0x41, 0x10, 0x1a, 0x10, 0x0c, 0xd2, 0x0d, 0x0b,]
        );
    }

    #[test]
    fn remap_preserves_padded_function_index_operands() {
        let body = [
            0x00, // local decl count
            0x10, 0x82, 0x80, 0x80, 0x80, 0x00, // call 2, padded to 5 bytes
            0x0b, // end
        ];
        let remapped = remap_function_body(&body, &|_| Ok(1)).unwrap();
        assert_eq!(
            remapped,
            vec![0x00, 0x10, 0x81, 0x80, 0x80, 0x80, 0x00, 0x0b]
        );
    }
}
