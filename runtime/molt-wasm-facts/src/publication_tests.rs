use std::borrow::Cow;

use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataCountSection, ElementSection, Elements, Function,
    FunctionSection, Instruction, Module, RefType, TableSection, TableType, TypeSection,
};
use wasmparser::{Parser, Payload};

use super::*;

fn module_with_callable_table(layout: Option<CallableTableLayout>) -> Vec<u8> {
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
    if let Some(layout) = layout {
        module.section(&CustomSection {
            name: Cow::Borrowed(CALLABLE_TABLE_LAYOUT_SECTION_NAME),
            data: Cow::Owned(crate::layout::encode_callable_table_layout(layout)),
        });
    }
    module.section(&CustomSection {
        name: Cow::Borrowed("preserved.metadata"),
        data: Cow::Borrowed(b"preserve-me"),
    });
    module.finish()
}

fn module_with_callable_slots(slots: &[u32]) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    for _ in slots {
        functions.function(0);
    }
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: slots.iter().copied().max().unwrap_or(0) as u64 + 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut elements = ElementSection::new();
    for (function_index, slot) in slots.iter().copied().enumerate() {
        elements.active(
            None,
            &ConstExpr::i32_const(slot as i32),
            Elements::Functions(Cow::Owned(vec![function_index as u32])),
        );
    }
    module.section(&elements);
    module.section(&DataCountSection { count: 0 });
    let mut code = CodeSection::new();
    for _ in slots {
        let mut body = Function::new([]);
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&code);
    module.finish()
}

#[test]
fn streaming_publication_replaces_sections_and_self_validates() {
    let base = module_with_callable_table(None);
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

#[test]
fn monolithic_publication_replaces_stale_pre_link_layout_from_final_elements() {
    let stale_layout = CallableTableLayout {
        fixed_prefix_base: 0,
        fixed_prefix_len: 0,
        finalized_app_base: 7,
        app_entry_count: 99,
    };
    let with_stale_layout = module_with_callable_table(Some(stale_layout));

    let published = publish_callable_table_attestation(&with_stale_layout, None)
        .expect("derive final monolithic layout");
    let facts = scan_wasm_link_facts(&published).expect("scan published module");

    assert_eq!(
        facts.callable_table_layout,
        Some(CallableTableLayout {
            fixed_prefix_base: 0,
            fixed_prefix_len: 0,
            finalized_app_base: 7,
            app_entry_count: 1,
        })
    );
}

#[test]
fn runtime_publication_accepts_owned_entries_beyond_required_fixed_prefix() {
    let base = module_with_callable_slots(&[10, 11, 12, 19]);
    let layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 2,
        finalized_app_base: 20,
        app_entry_count: 3,
    };
    let mut published = Vec::new();

    let facts = scan_and_write_callable_table_attestation(
        &base,
        Some(layout),
        CallableTableArtifactRole::Runtime,
        &mut published,
    )
    .expect("publish complete runtime ownership region");

    assert_eq!(facts.callable_table_entries.len(), 4);
    assert_eq!(facts.callable_table_layout, Some(layout));
}

#[test]
fn app_publication_derives_final_count_after_native_link_growth() {
    let base = module_with_callable_slots(&[20, 21, 22, 23]);
    let stale_pre_link_layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 2,
        finalized_app_base: 20,
        app_entry_count: 2,
    };
    let mut published = Vec::new();

    let facts = scan_and_write_callable_table_attestation(
        &base,
        Some(stale_pre_link_layout),
        CallableTableArtifactRole::App,
        &mut published,
    )
    .expect("derive final app count from linked active elements");

    assert_eq!(
        facts.callable_table_layout,
        Some(CallableTableLayout {
            app_entry_count: 4,
            ..stale_pre_link_layout
        })
    );
}

#[test]
fn app_publication_rejects_runtime_owned_prefix_entries() {
    let base = module_with_callable_slots(&[10, 20]);
    let layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 1,
        finalized_app_base: 20,
        app_entry_count: 1,
    };
    let mut published = Vec::new();

    let error = scan_and_write_callable_table_attestation(
        &base,
        Some(layout),
        CallableTableArtifactRole::App,
        &mut published,
    )
    .expect_err("split app must not republish runtime-owned fixed slots");

    assert!(error.contains("runtime-owned callable slot 10"), "{error}");
}

#[test]
fn monolithic_publication_derives_final_app_count_after_native_link_growth() {
    let base = module_with_callable_slots(&[10, 11, 12, 19, 20, 21, 22, 23]);
    let stale_pre_link_layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 2,
        finalized_app_base: 20,
        app_entry_count: 2,
    };
    let mut published = Vec::new();

    let facts = scan_and_write_callable_table_attestation(
        &base,
        Some(stale_pre_link_layout),
        CallableTableArtifactRole::Monolithic,
        &mut published,
    )
    .expect("derive final monolithic app count from linked active elements");

    assert_eq!(
        facts.callable_table_layout,
        Some(CallableTableLayout {
            app_entry_count: 4,
            ..stale_pre_link_layout
        })
    );
}

#[test]
fn monolithic_publication_accepts_runtime_entries_before_app_without_fixed_prefix() {
    let base = module_with_callable_slots(&[10, 20]);
    let layout = CallableTableLayout {
        fixed_prefix_base: 0,
        fixed_prefix_len: 0,
        finalized_app_base: 20,
        app_entry_count: 1,
    };
    let mut published = Vec::new();

    let facts = scan_and_write_callable_table_attestation(
        &base,
        Some(layout),
        CallableTableArtifactRole::Monolithic,
        &mut published,
    )
    .expect("publish runtime-owned entries before the finalized app boundary");

    assert_eq!(facts.callable_table_layout, Some(layout));
}

#[test]
fn publication_rejects_noncanonical_empty_fixed_prefix_base() {
    let base = module_with_callable_slots(&[20]);
    let layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 0,
        finalized_app_base: 20,
        app_entry_count: 1,
    };
    let mut published = Vec::new();

    let error = scan_and_write_callable_table_attestation(
        &base,
        Some(layout),
        CallableTableArtifactRole::App,
        &mut published,
    )
    .expect_err("reject noncanonical empty fixed prefix");

    assert!(
        error.contains("empty callable-table fixed prefix"),
        "{error}"
    );
}

#[test]
fn runtime_publication_rejects_incomplete_prefix_and_app_overlap() {
    let layout = CallableTableLayout {
        fixed_prefix_base: 10,
        fixed_prefix_len: 2,
        finalized_app_base: 20,
        app_entry_count: 1,
    };
    for (slots, message) in [
        (vec![10, 12], "runtime fixed prefix"),
        (vec![10, 11, 20], "reaches finalized app base"),
    ] {
        let mut published = Vec::new();
        let error = scan_and_write_callable_table_attestation(
            &module_with_callable_slots(&slots),
            Some(layout),
            CallableTableArtifactRole::Runtime,
            &mut published,
        )
        .expect_err("reject invalid runtime ownership");
        assert!(error.contains(message), "{error}");
    }
}
