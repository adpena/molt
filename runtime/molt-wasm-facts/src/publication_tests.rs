use std::borrow::Cow;

use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataCountSection, ElementSection, Elements, Function,
    FunctionSection, Instruction, Module, RefType, TableSection, TableType, TypeSection,
};
use wasmparser::{Parser, Payload};

use super::*;

fn module_with_callable_table() -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 8,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut elements = ElementSection::new();
    elements.active(
        None,
        &ConstExpr::i32_const(7),
        Elements::Functions(Cow::Owned(vec![0])),
    );
    module.section(&elements);
    module.section(&DataCountSection { count: 0 });
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    module.section(&CustomSection {
        name: Cow::Borrowed("preserved.metadata"),
        data: Cow::Borrowed(b"preserve-me"),
    });
    module.finish()
}

#[test]
fn streaming_publication_replaces_sections_and_self_validates() {
    let base = module_with_callable_table();
    let facts = scan_wasm_link_facts(&base).expect("scan unattested module");

    let published = publish_callable_table_attestation(&base, None).expect("publish attestation");
    let published_facts = scan_wasm_link_facts(&published).expect("validate published attestation");

    assert!(published_facts.callable_table_attestation_present);
    assert_eq!(
        published_facts.callable_table_entries,
        facts.callable_table_entries
    );
    assert_eq!(
        publish_callable_table_attestation(&published, None).expect("replace attestation"),
        published
    );
    let mut saw_data_count = false;
    let mut saw_preserved_custom = false;
    for payload in Parser::new(0).parse_all(&published) {
        match payload.expect("parse published section") {
            Payload::DataCountSection { count: 0, .. } => saw_data_count = true,
            Payload::CustomSection(reader) if reader.name() == "preserved.metadata" => {
                assert_eq!(reader.data(), b"preserve-me");
                saw_preserved_custom = true;
            }
            _ => {}
        }
    }
    assert!(saw_data_count);
    assert!(saw_preserved_custom);
}
