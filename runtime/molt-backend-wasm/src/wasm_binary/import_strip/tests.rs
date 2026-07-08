use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{
    CodeSection, ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, EntityType,
    ExportKind, ExportSection, Function, FunctionSection, ImportSection, Instruction, Module,
    RefType, StartSection, TableSection, TableType, TypeSection,
};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

use super::super::leb::read_u32_leb128;
use super::strip_unused_imports;

fn fixture_module() -> Vec<u8> {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);

    let mut imports = ImportSection::new();
    imports.import("env", "dead", EntityType::Function(0));
    imports.import("molt_runtime", "dead", EntityType::Function(0));
    imports.import("molt_runtime", "live", EntityType::Function(0));
    module.section(&imports);

    let mut funcs = FunctionSection::new();
    funcs.function(0);
    module.section(&funcs);

    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 2,
        maximum: None,
        shared: false,
    });
    module.section(&tables);

    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, 3);
    module.section(&exports);

    module.section(&StartSection { function_index: 3 });

    let offset = ConstExpr::i32_const(0);
    let mut elements = ElementSection::new();
    elements.segment(ElementSegment {
        mode: ElementMode::Active {
            table: None,
            offset: &offset,
        },
        elements: Elements::Functions(std::borrow::Cow::Owned(vec![2, 3])),
    });
    module.section(&elements);

    let mut codes = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(2));
    body.instruction(&Instruction::End);
    codes.function(&body);
    module.section(&codes);

    module.finish()
}

fn function_import_names(bytes: &[u8]) -> Vec<(String, String)> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::ImportSection(reader)) = payload {
            for import in reader.into_imports().flatten() {
                if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                    imports.push((import.module.to_string(), import.name.to_string()));
                }
            }
        }
    }
    imports
}

fn function_exports(bytes: &[u8]) -> BTreeMap<String, u32> {
    let mut exports = BTreeMap::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::ExportSection(reader)) = payload {
            for export in reader.into_iter().flatten() {
                if matches!(export.kind, ExternalKind::Func | ExternalKind::FuncExact) {
                    exports.insert(export.name.to_string(), export.index);
                }
            }
        }
    }
    exports
}

fn start_function(bytes: &[u8]) -> Option<u32> {
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::StartSection { func, .. }) = payload {
            return Some(func);
        }
    }
    None
}

fn element_function_indices(bytes: &[u8]) -> Vec<u32> {
    let mut indices = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::ElementSection(reader)) = payload {
            for element in reader.into_iter().flatten() {
                if let wasmparser::ElementItems::Functions(funcs) = element.items {
                    indices.extend(funcs.into_iter().flatten());
                }
            }
        }
    }
    indices
}

fn element_section_payload(bytes: &[u8]) -> Vec<u8> {
    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = bytes[pos];
        pos += 1;
        let (section_len, content_start) =
            read_u32_leb128(bytes, pos).expect("section size must parse");
        let content_end = content_start + section_len as usize;
        if section_id == 9 {
            return bytes[content_start..content_end].to_vec();
        }
        pos = content_end;
    }
    panic!("element section missing");
}

fn direct_call_indices(bytes: &[u8]) -> Vec<u32> {
    let mut calls = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Ok(Payload::CodeSectionEntry(body)) = payload
            && let Ok(mut ops) = body.get_operators_reader()
        {
            while let Ok(op) = ops.read() {
                if let wasmparser::Operator::Call { function_index } = op {
                    calls.push(function_index);
                }
            }
        }
    }
    calls
}

#[test]
fn strip_unused_imports_remaps_all_function_index_surfaces() {
    let mut unused = BTreeSet::new();
    unused.insert("dead".to_string());

    let stripped = strip_unused_imports(fixture_module(), &unused);
    wasmparser::Validator::new()
        .validate_all(&stripped)
        .expect("stripped module must validate");

    assert_eq!(
        function_import_names(&stripped),
        vec![
            ("env".to_string(), "dead".to_string()),
            ("molt_runtime".to_string(), "live".to_string()),
        ]
    );
    assert_eq!(function_exports(&stripped)["run"], 2);
    assert_eq!(start_function(&stripped), Some(2));
    assert_eq!(element_function_indices(&stripped), vec![1, 2]);
    assert_eq!(direct_call_indices(&stripped), vec![1]);
}

#[test]
fn strip_unused_imports_keeps_element_indices_relocation_padded() {
    let mut unused = BTreeSet::new();
    unused.insert("dead".to_string());

    let stripped = strip_unused_imports(fixture_module(), &unused);
    let payload = element_section_payload(&stripped);

    assert_eq!(
        payload,
        vec![
            0x01, // segment count
            0x00, // active table 0 segment
            0x41, 0x00, 0x0B, // i32.const 0; end
            0x02, // two function indices
            0x81, 0x80, 0x80, 0x80, 0x00, // remapped function index 1
            0x82, 0x80, 0x80, 0x80, 0x00, // remapped function index 2
        ]
    );
    assert_eq!(element_function_indices(&stripped), vec![1, 2]);
}

#[test]
#[should_panic(expected = "references removed function import index")]
fn strip_unused_imports_fails_if_removed_import_is_still_called() {
    let mut unused = BTreeSet::new();
    unused.insert("live".to_string());
    let _ = strip_unused_imports(fixture_module(), &unused);
}
