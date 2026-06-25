use crate::wasm_data::{DataRelocSite, DataSegmentInfo};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{
    ConstExpr, CustomSection, DataSymbolDefinition, ElementMode, ElementSection, ElementSegment,
    Elements, Encode, EntityType, ExportKind, ExportSection, Function, ImportSection, Instruction,
    LinkingSection, MemoryType, RefType, SymbolTable, TagKind, TagType, ValType,
};
use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

pub(crate) fn encode_u32_leb128_padded(mut value: u32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn encode_i32_sleb128_padded(mut value: i32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

pub(crate) fn emit_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        // Trap if the code path is actually reached at runtime.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x10);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::Call(func_index));
    }
}

/// Emit a simple N-arg import call: push args, call, store result.
pub(crate) fn emit_simple_call(
    func: &mut Function,
    reloc_enabled: bool,
    import_id: u32,
    arg_locals: &[u32],
    out_local: u32,
) {
    for &arg in arg_locals {
        func.instruction(&Instruction::LocalGet(arg));
    }
    emit_call(func, reloc_enabled, import_id);
    func.instruction(&Instruction::LocalSet(out_local));
}

/// Emit a `return_call` instruction (WASM tail calls proposal).
/// The callee's return value becomes the caller's return value without growing the stack.
pub(crate) fn emit_return_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x12); // return_call opcode
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::ReturnCall(func_index));
    }
}

pub(crate) fn emit_call_indirect(func: &mut Function, reloc_enabled: bool, ty: u32, table: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(11);
        bytes.push(0x11);
        encode_u32_leb128_padded(ty, &mut bytes);
        encode_u32_leb128_padded(table, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::CallIndirect {
            type_index: ty,
            table_index: table,
        });
    }
}

pub(crate) fn emit_i32_const(func: &mut Function, reloc_enabled: bool, value: i32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x41);
        encode_i32_sleb128_padded(value, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::I32Const(value));
    }
}

pub(crate) fn emit_ref_func(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0xD2);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::RefFunc(func_index));
    }
}

pub(crate) fn emit_table_index_i32(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_i32_const(func, reloc_enabled, table_index as i32);
}

pub(crate) fn emit_table_index_i64(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_table_index_i32(func, reloc_enabled, table_index);
    func.instruction(&Instruction::I64ExtendI32U);
}

pub(crate) fn const_expr_i32_const_padded(value: i32) -> ConstExpr {
    let mut bytes = Vec::with_capacity(6);
    bytes.push(0x41);
    encode_i32_sleb128_padded(value, &mut bytes);
    ConstExpr::raw(bytes)
}

#[derive(Clone, Copy)]
enum PendingReloc {
    Function { offset: u32, func_index: u32 },
    Type { offset: u32, type_index: u32 },
    DataAddr { offset: u32, segment_index: u32 },
}

#[derive(Clone, Copy)]
struct RelocEntry {
    ty: u8,
    offset: u32,
    index: u32,
    addend: i32,
}

fn encode_reloc_section(
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

fn append_custom_section(bytes: &mut Vec<u8>, section: &impl Encode) {
    bytes.push(0);
    section.encode(bytes);
}

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

pub(crate) fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<String> = Vec::new();
    let mut func_exports: BTreeMap<u32, String> = BTreeMap::new();
    let mut func_import_count = 0u32;
    let mut defined_func_count = 0u32;
    let mut table_import_count = 0u32;
    let mut table_defined_count = 0u32;
    let mut code_section_start = None;
    let mut code_section_index = None;
    let mut data_section_index = None;
    let mut element_section_index = None;
    let mut func_body_starts: Vec<usize> = Vec::new();
    let mut pending_code: Vec<PendingReloc> = Vec::new();
    let mut pending_data: Vec<PendingReloc> = Vec::new();
    let mut pending_elem: Vec<PendingReloc> = Vec::new();
    let mut section_index = 0u32;

    let mut parse_failed = false;
    for payload in Parser::new(0).parse_all(&bytes) {
        let payload = match payload {
            Ok(payload) => payload,
            Err(_) => {
                parse_failed = true;
                break;
            }
        };
        match payload {
            Payload::TypeSection(_) => {
                section_index += 1;
            }
            Payload::ImportSection(reader) => {
                section_index += 1;
                for import in reader.into_imports().flatten() {
                    match import.ty {
                        TypeRef::Func(_) => {
                            func_imports.push(import.name.to_string());
                            func_import_count += 1;
                        }
                        TypeRef::Table(_) => {
                            table_import_count += 1;
                        }
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                defined_func_count = reader.count();
                section_index += 1;
            }
            Payload::TableSection(reader) => {
                table_defined_count = reader.count();
                section_index += 1;
            }
            Payload::MemorySection(_) => {
                section_index += 1;
            }
            Payload::GlobalSection(_) => {
                section_index += 1;
            }
            Payload::ExportSection(reader) => {
                for export in reader.into_iter().flatten() {
                    if export.kind == ExternalKind::Func {
                        func_exports.insert(export.index, export.name.to_string());
                    }
                }
                section_index += 1;
            }
            Payload::StartSection { .. } => {
                section_index += 1;
            }
            Payload::ElementSection(reader) => {
                let element_section_start = reader.range().start;
                element_section_index = Some(section_index);
                section_index += 1;
                for element in reader.into_iter().flatten() {
                    if let ElementItems::Functions(funcs) = element.items {
                        for func in funcs.into_iter_with_offsets().flatten() {
                            let (pos, func_index) = func;
                            let offset = (pos.saturating_sub(element_section_start)) as u32;
                            pending_elem.push(PendingReloc::Function { offset, func_index });
                        }
                    }
                }
            }
            Payload::CodeSectionStart { range, .. } => {
                code_section_start = Some(range.start);
                code_section_index = Some(section_index);
                section_index += 1;
            }
            Payload::CodeSectionEntry(body) => {
                func_body_starts.push(body.range().start);
                if let Ok(mut ops) = body.get_operators_reader() {
                    while let Ok((op, op_offset)) = ops.read_with_offset() {
                        let start = match code_section_start {
                            Some(start) => start,
                            None => break,
                        };
                        match op {
                            Operator::Call { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            Operator::CallIndirect { type_index, .. } => {
                                let type_offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Type {
                                    offset: type_offset,
                                    type_index,
                                });
                            }
                            Operator::RefFunc { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Payload::DataSection(reader) => {
                let data_section_start = reader.range().start;
                data_section_index = Some(section_index);
                section_index += 1;
                for (segment_index, data) in reader.into_iter().enumerate() {
                    if let Ok(data) = data
                        && let DataKind::Active { offset_expr, .. } = data.kind
                    {
                        let mut ops = offset_expr.get_operators_reader();
                        if let Ok((Operator::I32Const { .. }, op_offset)) = ops.read_with_offset() {
                            let offset = (op_offset + 1).saturating_sub(data_section_start) as u32;
                            pending_data.push(PendingReloc::DataAddr {
                                offset,
                                segment_index: segment_index as u32,
                            });
                        }
                    }
                }
            }
            Payload::DataCountSection { .. } => {
                section_index += 1;
            }
            _ => {}
        }
    }
    if parse_failed {
        return bytes;
    }

    let code_section_start = match code_section_start {
        Some(start) => start,
        None => return bytes,
    };
    let code_section_index = match code_section_index {
        Some(index) => index,
        None => return bytes,
    };
    let data_section_index = data_section_index;

    for site in data_relocs {
        let def_index = site.func_index.saturating_sub(func_import_count) as usize;
        if let Some(body_start) = func_body_starts.get(def_index) {
            let offset = (body_start.saturating_sub(code_section_start) as u32)
                .saturating_add(site.offset_in_func);
            pending_code.push(PendingReloc::DataAddr {
                offset,
                segment_index: site.segment_index,
            });
        }
    }

    let total_funcs = func_import_count + defined_func_count;
    let mut func_symbol_map = vec![0u32; total_funcs as usize];
    let mut data_symbol_map = vec![0u32; data_segments.len()];
    let mut symbol_index = 0u32;

    let mut sym_tab = SymbolTable::new();
    let mut import_names: Vec<String> = Vec::new();
    for (idx, name) in func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = format!("molt_{name}");
        import_names.push(symbol_name);
        let name_ref = import_names.last().unwrap();
        sym_tab.function(flags, idx as u32, Some(name_ref));
        func_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }
    let mut func_names: Vec<String> = Vec::new();
    for def_idx in 0..defined_func_count {
        let func_index = func_import_count + def_idx;
        let export_name = func_exports.get(&func_index).cloned();
        // Keep linker symbol names module-scoped so linked output/runtime objects
        // cannot accidentally alias local function symbols with identical names.
        // Preserve explicit call_indirect export symbols because wasm_link.py
        // resolves/aliases those by name for runtime ABI wiring.
        let name = match export_name.as_deref() {
            Some("molt_main") | Some("molt_table_init") => export_name.clone().unwrap_or_default(),
            Some(exported) if exported.starts_with("molt_call_indirect") => exported.to_string(),
            Some(_) => format!("__molt_output_export_{func_index}"),
            None => format!("__molt_output_fn_{func_index}"),
        };
        func_names.push(name);
        let name_ref = func_names.last().unwrap();
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        sym_tab.function(flags, func_index, Some(name_ref));
        func_symbol_map[func_index as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        sym_tab.table(flags, table_idx, None);
        symbol_index += 1;
    }
    let mut table_names: Vec<String> = Vec::new();
    for table_idx in 0..table_defined_count {
        let index = table_import_count + table_idx;
        let name = format!("__molt_output_table_{index}");
        table_names.push(name);
        let name_ref = table_names.last().unwrap();
        sym_tab.table(0, index, Some(name_ref));
        symbol_index += 1;
    }

    let mut data_names: Vec<String> = Vec::new();
    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_output_data_{idx}");
        data_names.push(name);
        let name_ref = data_names.last().unwrap();
        sym_tab.data(
            0,
            name_ref,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }

    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();
    for reloc in pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = func_symbol_map.get(func_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 0,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::Type { offset, type_index } => {
                code_entries.push(RelocEntry {
                    ty: 6,
                    offset,
                    index: type_index,
                    addend: 0,
                });
            }
            PendingReloc::DataAddr {
                offset,
                segment_index,
            } => {
                if let Some(index) = data_symbol_map.get(segment_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 4,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
        }
    }

    for reloc in pending_data {
        if let PendingReloc::DataAddr {
            offset,
            segment_index,
        } = reloc
            && let Some(index) = data_symbol_map.get(segment_index as usize)
        {
            data_entries.push(RelocEntry {
                ty: 4,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    for reloc in pending_elem {
        if let PendingReloc::Function { offset, func_index } = reloc
            && let Some(index) = func_symbol_map.get(func_index as usize)
        {
            elem_entries.push(RelocEntry {
                ty: 0,
                offset,
                index: *index,
                addend: 0,
            });
        }
    }

    code_entries.sort_by_key(|entry| entry.offset);
    data_entries.sort_by_key(|entry| entry.offset);
    elem_entries.sort_by_key(|entry| entry.offset);

    let mut linking = LinkingSection::new();
    linking.symbol_table(&sym_tab);
    append_custom_section(&mut bytes, &linking);
    if !code_entries.is_empty() {
        let reloc_code = encode_reloc_section("reloc.CODE", code_section_index, &code_entries);
        append_custom_section(&mut bytes, &reloc_code);
    }
    if !data_entries.is_empty()
        && let Some(index) = data_section_index
    {
        let reloc_data = encode_reloc_section("reloc.DATA", index, &data_entries);
        append_custom_section(&mut bytes, &reloc_data);
    }
    if !elem_entries.is_empty()
        && let Some(index) = element_section_index
    {
        let reloc_elem = encode_reloc_section("reloc.ELEM", index, &elem_entries);
        append_custom_section(&mut bytes, &reloc_elem);
    }

    bytes
}
