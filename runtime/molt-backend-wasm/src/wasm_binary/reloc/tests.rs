use super::{add_reloc_sections, symbols::is_manifest_call_indirect_import_name};
use crate::wasm_abi::{NATIVE_CALLABLE_IMPORT_MODULE, RUNTIME_IMPORT_MODULE, TypeSectionExt};
use crate::wasm_binary::strip_unused_imports;
use crate::wasm_data::WasmDataSegments;
use std::collections::BTreeSet;
use wasm_encoder::{
    CodeSection, EntityType, Function, FunctionSection, ImportSection, Instruction, Module,
    TypeSection,
};
use wasmparser::{Parser, Payload};

fn read_varuint(data: &[u8], mut offset: usize) -> (u32, usize) {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        let byte = data[offset];
        offset += 1;
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (result, offset);
        }
        shift += 7;
    }
}

fn bytes_contain_ascii(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn code_body_ranges(wasm: &[u8]) -> (usize, Vec<std::ops::Range<usize>>) {
    let mut code_start = None;
    let mut ranges = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm payload") {
            Payload::CodeSectionStart { range, .. } => {
                code_start = Some(range.start);
            }
            Payload::CodeSectionEntry(body) => {
                ranges.push(body.range());
            }
            _ => {}
        }
    }
    (code_start.expect("code section start"), ranges)
}

fn reloc_code_memory_addr_offsets(wasm: &[u8]) -> Vec<u32> {
    let mut offsets = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(section) = payload.expect("valid wasm payload") else {
            continue;
        };
        if section.name() != "reloc.CODE" {
            continue;
        }
        let data = section.data();
        let (_, cursor) = read_varuint(data, 0);
        let (count, mut cursor) = read_varuint(data, cursor);
        for _ in 0..count {
            let ty = data[cursor];
            cursor += 1;
            let (offset, next) = read_varuint(data, cursor);
            cursor = next;
            let (_, next) = read_varuint(data, cursor);
            cursor = next;
            if matches!(ty, 4 | 5) {
                let (_, next) = read_varuint(data, cursor);
                cursor = next;
            }
            if ty == 4 {
                offsets.push(offset);
            }
        }
    }
    offsets
}

#[test]
fn call_indirect_symbol_preservation_uses_manifest_membership() {
    assert!(is_manifest_call_indirect_import_name("molt_call_indirect0"));
    assert!(is_manifest_call_indirect_import_name(
        "molt_call_indirect13"
    ));
    assert!(!is_manifest_call_indirect_import_name(
        "molt_call_indirect99"
    ));
    assert!(!is_manifest_call_indirect_import_name("molt_call_indirect"));
}

#[test]
fn reloc_symbol_table_preserves_native_callable_import_symbols() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "types_bootstrap",
        EntityType::Function(0),
    );
    imports.import(
        NATIVE_CALLABLE_IMPORT_MODULE,
        "PyInit__nd_image",
        EntityType::Function(0),
    );

    let mut funcs = FunctionSection::new();
    funcs.function(0);

    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(0));
    body.instruction(&Instruction::Call(1));
    body.instruction(&Instruction::End);
    let mut codes = CodeSection::new();
    codes.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);

    let relocated = add_reloc_sections(module.finish(), &[], &[]);
    assert!(
        bytes_contain_ascii(&relocated, "PyInit__nd_image"),
        "native callable import symbol must be preserved for object-closure linking"
    );
}

#[test]
#[should_panic(expected = "unsupported WASM function import module")]
fn reloc_symbol_table_rejects_unknown_function_import_modules() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import("env", "PyInit__nd_image", EntityType::Function(0));

    let mut funcs = FunctionSection::new();
    funcs.function(0);

    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(0));
    body.instruction(&Instruction::End);
    let mut codes = CodeSection::new();
    codes.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);

    let _ = add_reloc_sections(module.finish(), &[], &[]);
}

#[test]
fn data_reloc_sites_follow_defined_body_ordinal_after_import_strip() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "types_bootstrap",
        EntityType::Function(0),
    );
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "abc_bootstrap",
        EntityType::Function(0),
    );

    let mut funcs = FunctionSection::new();
    funcs.function(0);
    funcs.function(0);

    let mut codes = CodeSection::new();
    let mut first = Function::new([]);
    first.instruction(&Instruction::End);
    codes.function(&first);

    let mut data_segments = WasmDataSegments::new(64 * 1024 * 1024);
    let data_ref = data_segments.add_segment(true, b"molt");
    let mut second = Function::new([]);
    data_segments.emit_ptr_i32(true, 1, &mut second, data_ref);
    second.instruction(&Instruction::Drop);
    second.instruction(&Instruction::End);
    codes.function(&second);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);
    module.section(data_segments.section());

    let mut unused = BTreeSet::new();
    unused.insert("types_bootstrap".to_string());
    let stripped = strip_unused_imports(module.finish(), &unused);
    let relocated = add_reloc_sections(stripped, data_segments.segments(), data_segments.relocs());

    let (code_start, body_ranges) = code_body_ranges(&relocated);
    assert_eq!(body_ranges.len(), 2);
    let second_body = &body_ranges[1];
    let second_start = (second_body.start - code_start) as u32;
    let second_end = (second_body.end - code_start) as u32;

    let offsets = reloc_code_memory_addr_offsets(&relocated);
    assert_eq!(offsets.len(), 1);
    assert!(
        (second_start..second_end).contains(&offsets[0]),
        "data relocation offset {} must target second defined body range {}..{}",
        offsets[0],
        second_start,
        second_end,
    );
}
