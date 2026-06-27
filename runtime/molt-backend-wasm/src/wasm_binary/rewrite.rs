use std::borrow::Cow;
use std::collections::BTreeSet;

use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, EntityType,
    ExportKind, ExportSection, ImportSection, MemoryType, RefType, TagKind, TagType, ValType,
};
use wasmparser::{ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

// ---------------------------------------------------------------------------
// Dead import elimination (post-serialization rewrite)
// ---------------------------------------------------------------------------
//
// After the WASM module is serialized, this pass removes function imports that
// were registered but never referenced during code emission.  Removing imports
// shifts all function indices above the removed slots, so we must remap every
// function-index reference in the module:
//
//   - Import section:  rebuilt without the dead entries
//   - Code section:    call / return_call / ref.func operands remapped
//   - Element section: function index entries remapped
//   - Export section:   function index entries remapped
//   - Start section:   function index remapped (if present)
//
// The approach: parse the binary section by section using `wasmparser`,
// rebuild affected sections using `wasm_encoder`, and copy unchanged
// sections verbatim.

/// Read a single unsigned LEB128 value from `data[offset..]`.
/// Returns `(value, new_offset)`.
fn read_u32_leb128(data: &[u8], mut offset: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    loop {
        let byte = data[offset];
        offset += 1;
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, offset)
}

/// Encode a u32 as unsigned LEB128 and append to `out`.
fn encode_u32_leb128(mut value: u32, out: &mut Vec<u8>) {
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

/// Strip unused function imports from a serialized WASM module.
///
/// `unused_names` contains the import field names (within the "molt_runtime"
/// module) that should be removed.  Returns a new WASM binary with those
/// imports excised and all function indices remapped accordingly.
pub(crate) fn strip_unused_imports(bytes: Vec<u8>, unused_names: &BTreeSet<String>) -> Vec<u8> {
    // Phase 1: Parse the import section to build the old→new index remap.
    let mut func_import_count: u32 = 0;
    let mut remap: Vec<u32> = Vec::new();
    let mut removed_count: u32 = 0;

    {
        let mut parse_ok = true;
        for payload in Parser::new(0).parse_all(&bytes) {
            let payload = match payload {
                Ok(p) => p,
                Err(_) => {
                    parse_ok = false;
                    break;
                }
            };
            if let Payload::ImportSection(reader) = payload {
                let mut new_idx: u32 = 0;
                for import in reader.into_imports().flatten() {
                    let is_func = matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_));
                    if is_func {
                        func_import_count += 1;
                        if unused_names.contains(import.name) {
                            remap.push(u32::MAX);
                            removed_count += 1;
                        } else {
                            remap.push(new_idx);
                            new_idx += 1;
                        }
                    }
                }
                break;
            }
        }
        if !parse_ok {
            return bytes;
        }
    }

    if removed_count == 0 {
        return bytes;
    }

    let remap_func_index = |old: u32| -> u32 {
        if old < func_import_count {
            remap[old as usize]
        } else {
            old - removed_count
        }
    };

    // Phase 2: Rebuild the module by reading raw section bytes.
    // WASM binary format: magic (4 bytes) + version (4 bytes) + sections.
    // Each section: section_id (1 byte) + u32 LEB128 size + content bytes.
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..8]); // header

    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = bytes[pos];
        pos += 1;
        let (section_size, content_start) = read_u32_leb128(&bytes, pos);
        let content_end = content_start + section_size as usize;
        let section_bytes = &bytes[content_start..content_end];

        match section_id {
            // Import section (2): rebuild without dead entries.
            2 => {
                let mut section = ImportSection::new();
                // Re-parse just this section.
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ImportSection(reader) = payload {
                        for import in reader.into_imports().flatten() {
                            let is_func = matches!(import.ty, TypeRef::Func(_));
                            if is_func && unused_names.contains(import.name) {
                                continue;
                            }
                            let entity_ty = match import.ty {
                                TypeRef::Func(idx) => EntityType::Function(idx),
                                TypeRef::Table(t) => EntityType::Table(wasm_encoder::TableType {
                                    element_type: convert_ref_type(t.element_type),
                                    table64: t.table64,
                                    minimum: t.initial,
                                    maximum: t.maximum,
                                    shared: t.shared,
                                }),
                                TypeRef::Memory(m) => EntityType::Memory(MemoryType {
                                    minimum: m.initial,
                                    maximum: m.maximum,
                                    memory64: m.memory64,
                                    shared: m.shared,
                                    page_size_log2: m.page_size_log2,
                                }),
                                TypeRef::Global(g) => {
                                    EntityType::Global(wasm_encoder::GlobalType {
                                        val_type: convert_val_type(g.content_type),
                                        mutable: g.mutable,
                                        shared: g.shared,
                                    })
                                }
                                TypeRef::Tag(t) => EntityType::Tag(TagType {
                                    kind: TagKind::Exception,
                                    func_type_idx: t.func_type_idx,
                                }),
                                TypeRef::FuncExact(idx) => EntityType::Function(idx),
                            };
                            section.import(import.module, import.name, entity_ty);
                        }
                        break;
                    }
                }
                out.push(2); // import section id
                section.encode(&mut out);
            }

            // Export section (7): remap function indices.
            7 => {
                let mut section = ExportSection::new();
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ExportSection(reader) = payload {
                        for export in reader.into_iter().flatten() {
                            let kind = match export.kind {
                                ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
                                ExternalKind::Table => ExportKind::Table,
                                ExternalKind::Memory => ExportKind::Memory,
                                ExternalKind::Global => ExportKind::Global,
                                ExternalKind::Tag => ExportKind::Tag,
                            };
                            let index = if matches!(
                                export.kind,
                                ExternalKind::Func | ExternalKind::FuncExact
                            ) {
                                remap_func_index(export.index)
                            } else {
                                export.index
                            };
                            section.export(export.name, kind, index);
                        }
                        break;
                    }
                }
                out.push(7); // export section id
                section.encode(&mut out);
            }

            // Element section (9): remap function indices.
            9 => {
                let mut section = ElementSection::new();
                for payload in Parser::new(0).parse_all(&bytes) {
                    let Ok(payload) = payload else { break };
                    if let Payload::ElementSection(reader) = payload {
                        for element in reader.into_iter().flatten() {
                            if let ElementItems::Functions(funcs) = element.items {
                                let indices: Vec<u32> =
                                    funcs.into_iter().flatten().map(&remap_func_index).collect();
                                match element.kind {
                                    wasmparser::ElementKind::Active {
                                        table_index,
                                        offset_expr,
                                    } => {
                                        let mut ops = offset_expr.get_operators_reader();
                                        let offset_val =
                                            if let Ok(Operator::I32Const { value }) = ops.read() {
                                                value
                                            } else {
                                                0
                                            };
                                        let c = ConstExpr::i32_const(offset_val);
                                        let table = table_index.filter(|&t| t != 0);
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Active { table, offset: &c },
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                    wasmparser::ElementKind::Passive => {
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Passive,
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                    wasmparser::ElementKind::Declared => {
                                        section.segment(ElementSegment {
                                            mode: ElementMode::Declared,
                                            elements: Elements::Functions(Cow::Owned(indices)),
                                        });
                                    }
                                };
                            }
                        }
                        break;
                    }
                }
                out.push(9); // element section id
                section.encode(&mut out);
            }

            // Code section (10): remap function indices in instructions.
            10 => {
                let new_code = remap_code_section(section_bytes, &remap_func_index);
                out.push(10);
                encode_u32_leb128(new_code.len() as u32, &mut out);
                out.extend_from_slice(&new_code);
            }

            // Start section (8): remap start function index.
            8 => {
                let (old_idx, _) = read_u32_leb128(section_bytes, 0);
                let new_idx = remap_func_index(old_idx);
                let mut body = Vec::new();
                encode_u32_leb128(new_idx, &mut body);
                out.push(8);
                encode_u32_leb128(body.len() as u32, &mut out);
                out.extend_from_slice(&body);
            }

            // All other sections: copy verbatim.
            _ => {
                out.push(section_id);
                // Copy the original LEB128 size + content.
                out.extend_from_slice(&bytes[pos..content_end]);
            }
        }

        pos = content_end;
    }

    if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1") {
        eprintln!(
            "[molt-wasm-import-strip] eliminated {removed_count} unused imports \
             ({func_import_count} → {}), binary {} → {} bytes (saved {} bytes)",
            func_import_count - removed_count,
            bytes.len(),
            out.len(),
            bytes.len().saturating_sub(out.len()),
        );
    }

    out
}

/// Validate that a WASM binary has well-formed section structure.
/// Returns true if all section IDs are valid and sizes don't overflow.
pub(crate) fn validate_wasm_sections(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    if &bytes[0..4] != b"\x00asm" {
        return false;
    }
    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = bytes[pos];
        // Valid: 0 (custom), 1-12 (standard), 13 (tag from exception handling).
        if section_id > 13 {
            return false;
        }
        pos += 1;
        if pos >= bytes.len() {
            return false;
        }
        let (size, new_pos) = read_u32_leb128(bytes, pos);
        pos = new_pos + size as usize;
        if pos > bytes.len() {
            return false;
        }
    }
    pos == bytes.len()
}

/// Convert a wasmparser RefType to a wasm_encoder RefType.
fn convert_ref_type(ty: wasmparser::RefType) -> RefType {
    if ty.is_func_ref() {
        RefType::FUNCREF
    } else if ty.is_extern_ref() {
        RefType::EXTERNREF
    } else {
        RefType::FUNCREF // fallback
    }
}

/// Convert a wasmparser ValType to a wasm_encoder ValType.
fn convert_val_type(ty: wasmparser::ValType) -> ValType {
    match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        wasmparser::ValType::V128 => ValType::V128,
        wasmparser::ValType::Ref(r) => ValType::Ref(convert_ref_type(r)),
    }
}

/// Rewrite the code section content (after the section header) with remapped
/// function indices.  Returns the new section body (count + function bodies).
///
/// WASM opcodes that reference function indices:
///   - call (0x10): u32 function_index
///   - return_call (0x12): u32 function_index
///   - ref.func (0xD2): u32 function_index
fn remap_code_section(section_content: &[u8], remap: &dyn Fn(u32) -> u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(section_content.len());

    // The section content starts with the function count.
    let (count, mut offset) = read_u32_leb128(section_content, 0);
    encode_u32_leb128(count, &mut out);

    // Process each function body.
    for _ in 0..count {
        let (body_size, body_start) = read_u32_leb128(section_content, offset);
        let body_end = body_start + body_size as usize;
        let body = &section_content[body_start..body_end];

        // Rewrite function indices within this function body.
        let new_body = remap_function_body(body, remap);

        encode_u32_leb128(new_body.len() as u32, &mut out);
        out.extend_from_slice(&new_body);

        offset = body_end;
    }

    out
}

/// Rewrite a single function body, remapping call/return_call/ref.func indices.
///
/// Function body format: locals declarations + instruction sequence.
/// We skip over the locals section and scan the instruction bytes.
fn remap_function_body(body: &[u8], remap: &dyn Fn(u32) -> u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut pos: usize = 0;

    // Skip local declarations.
    let (local_decl_count, new_pos) = read_u32_leb128(body, pos);
    // Copy the local declarations verbatim.
    let locals_start = pos;
    pos = new_pos;
    for _ in 0..local_decl_count {
        // Each declaration: count (u32 LEB128) + type (1 byte).
        let (_count, p) = read_u32_leb128(body, pos);
        pos = p + 1; // +1 for the type byte
    }
    // Copy everything up to here (local declarations).
    out.extend_from_slice(&body[locals_start..pos]);

    // Now scan the instruction stream.
    while pos < body.len() {
        let opcode = body[pos];
        match opcode {
            // call (0x10) or return_call (0x12): remap the function index.
            0x10 | 0x12 => {
                out.push(opcode);
                pos += 1;
                let (old_idx, new_pos) = read_u32_leb128(body, pos);
                let new_idx = remap(old_idx);
                encode_u32_leb128(new_idx, &mut out);
                pos = new_pos;
            }
            // ref.func (0xD2): remap the function index.
            0xD2 => {
                out.push(opcode);
                pos += 1;
                let (old_idx, new_pos) = read_u32_leb128(body, pos);
                let new_idx = remap(old_idx);
                encode_u32_leb128(new_idx, &mut out);
                pos = new_pos;
            }
            // All other opcodes: copy byte by byte.
            // For multi-byte opcodes (0xFC prefix, 0xFE prefix), we need to
            // be careful not to accidentally interpret operands as opcodes.
            // However, function index operands only appear in the three
            // opcodes above, so we just need to skip operands correctly.
            //
            // Fortunately, WASM instruction encoding is self-describing:
            // each opcode has a fixed operand format.  We handle the common
            // cases and for anything else, copy byte-by-byte (which is safe
            // because no other single-byte opcode's operand looks like 0x10,
            // 0x12, or 0xD2 in a way that would be misinterpreted — WASM
            // LEB128 operands are self-delimiting).
            //
            // IMPORTANT: We must handle opcodes with LEB128 operands to avoid
            // misinterpreting operand bytes as opcodes.
            _ => {
                out.push(opcode);
                pos += 1;
                // Copy operands for known opcodes to avoid misparse.
                match opcode {
                    // Control flow with block types
                    0x02..=0x04 => {
                        // block/loop/if: blocktype (signed LEB128)
                        copy_sleb128(body, &mut pos, &mut out);
                    }
                    // br / br_if: label index (u32 LEB128)
                    0x0C | 0x0D => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // br_table: vec(label) + default_label
                    0x0E => {
                        let (count, new_pos) = read_u32_leb128(body, pos);
                        // Re-encode count
                        let start = pos;
                        pos = new_pos;
                        out.extend_from_slice(&body[start..pos]);
                        // Skip count+1 label indices (but we already wrote count).
                        // Actually, we need to just copy them. Let me redo this.
                        // We wrote count in the extend above. Now copy count+1 labels.
                        for _ in 0..=count {
                            copy_uleb128(body, &mut pos, &mut out);
                        }
                    }
                    // call_indirect: type_index + table_index
                    0x11 => {
                        copy_uleb128(body, &mut pos, &mut out); // type_index
                        copy_uleb128(body, &mut pos, &mut out); // table_index
                    }
                    // Variable access: local.get/set/tee (0x20-0x22)
                    0x20..=0x22 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // Global access: global.get/set (0x23-0x24)
                    0x23 | 0x24 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // Memory instructions (0x28-0x3E): memarg (align + offset)
                    0x28..=0x3E => {
                        copy_uleb128(body, &mut pos, &mut out); // align
                        copy_uleb128(body, &mut pos, &mut out); // offset
                    }
                    // memory.size / memory.grow (0x3F, 0x40): memory index
                    0x3F | 0x40 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // i32.const (0x41): signed LEB128
                    0x41 => {
                        copy_sleb128(body, &mut pos, &mut out);
                    }
                    // i64.const (0x42): signed LEB128 (64-bit)
                    0x42 => {
                        copy_sleb128_64(body, &mut pos, &mut out);
                    }
                    // f32.const (0x43): 4 bytes
                    0x43 => {
                        out.extend_from_slice(&body[pos..pos + 4]);
                        pos += 4;
                    }
                    // f64.const (0x44): 8 bytes
                    0x44 => {
                        out.extend_from_slice(&body[pos..pos + 8]);
                        pos += 8;
                    }
                    // 0xFC prefix: multi-byte opcodes
                    0xFC => {
                        let (sub_opcode, new_pos) = read_u32_leb128(body, pos);
                        encode_u32_leb128(sub_opcode, &mut out);
                        pos = new_pos;
                        match sub_opcode {
                            // memory.init: data_idx + mem_idx
                            8 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // data.drop: data_idx
                            9 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // memory.copy: mem_idx + mem_idx
                            10 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // memory.fill: mem_idx
                            11 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.init: elem_idx + table_idx
                            12 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // elem.drop: elem_idx
                            13 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.copy: table_idx + table_idx
                            14 => {
                                copy_uleb128(body, &mut pos, &mut out);
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // table.grow/size/fill: table_idx
                            15..=17 => {
                                copy_uleb128(body, &mut pos, &mut out);
                            }
                            // i32.trunc_sat_f32_s (0), etc. (0-7): no operands
                            0..=7 => {}
                            _ => {
                                // Unknown 0xFC sub-opcode: best-effort, no extra operands.
                            }
                        }
                    }
                    // try_table (0x06 in exception handling): blocktype + catch clauses
                    0x06 => {
                        copy_sleb128(body, &mut pos, &mut out); // blocktype
                        let (catch_count, new_pos) = read_u32_leb128(body, pos);
                        let start = pos;
                        pos = new_pos;
                        out.extend_from_slice(&body[start..pos]);
                        for _ in 0..catch_count {
                            // catch clause: catch_kind (1 byte) + optional tag_index + label
                            let catch_kind = body[pos];
                            out.push(catch_kind);
                            pos += 1;
                            match catch_kind {
                                // catch / catch_ref: tag_index + label
                                0x00 | 0x01 => {
                                    copy_uleb128(body, &mut pos, &mut out);
                                    copy_uleb128(body, &mut pos, &mut out);
                                }
                                // catch_all / catch_all_ref: label only
                                0x02 | 0x03 => {
                                    copy_uleb128(body, &mut pos, &mut out);
                                }
                                _ => {}
                            }
                        }
                    }
                    // throw (0x08): tag_index
                    0x08 => {
                        copy_uleb128(body, &mut pos, &mut out);
                    }
                    // throw_ref (0x0A): no operands
                    // All other single-byte opcodes (arithmetic, comparison,
                    // unreachable, nop, return, drop, select, etc.) have no
                    // LEB128 operands and are handled by the default copy.
                    _ => {}
                }
            }
        }
    }

    out
}

/// Copy a single unsigned LEB128 value from `src[pos..]` to `out`.
fn copy_uleb128(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}

/// Copy a single signed LEB128 value (32-bit) from `src[pos..]` to `out`.
fn copy_sleb128(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}

/// Copy a single signed LEB128 value (64-bit) from `src[pos..]` to `out`.
fn copy_sleb128_64(src: &[u8], pos: &mut usize, out: &mut Vec<u8>) {
    loop {
        let byte = src[*pos];
        out.push(byte);
        *pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
}
