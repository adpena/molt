use std::borrow::Cow;

use wasm_encoder::Encode;
use wasm_encoder::{
    ArrayType, CodeSection, Component, CompositeInnerType as EncoderCompositeInnerType,
    CompositeType as EncoderCompositeType, ConstExpr, CustomSection, DataSection, ElementMode,
    ElementSection, ElementSegment, Elements, EntityType, ExportKind, ExportSection, FieldType,
    FuncType, Function, FunctionSection, GlobalSection, GlobalType, HeapType, ImportSection,
    Instruction, Module, RefType, StorageType, SubType, TableSection, TableType, TypeSection,
    ValType,
};

use super::*;

fn module_with_bodies(bodies: &[Vec<Instruction<'static>>]) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    for instructions in bodies {
        functions.function(0);
        let mut body = Function::new([]);
        for instruction in instructions {
            body.instruction(instruction);
        }
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut elements = ElementSection::new();
    elements.segment(ElementSegment {
        mode: ElementMode::Passive,
        elements: Elements::Functions(Cow::Owned(
            (0..u32::try_from(bodies.len()).expect("fixture body count fits u32")).collect(),
        )),
    });
    module.section(&elements);
    module.section(&code);
    module.finish()
}

fn callable_table_attestation(slot: u32, function_index: u32, type_index: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    1u32.encode(&mut payload);
    1u32.encode(&mut payload);
    1u32.encode(&mut payload);
    type_index.encode(&mut payload);
    0u32.encode(&mut payload);
    0u32.encode(&mut payload);
    1u32.encode(&mut payload);
    slot.encode(&mut payload);
    function_index.encode(&mut payload);
    type_index.encode(&mut payload);
    0u32.encode(&mut payload);
    payload
}

fn module_with_callable_table(attestations: &[Vec<u8>]) -> Vec<u8> {
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
    let offset = ConstExpr::i32_const(7);
    let mut elements = ElementSection::new();
    elements.active(None, &offset, Elements::Functions(Cow::Owned(vec![0])));
    module.section(&elements);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    for attestation in attestations {
        module.section(&CustomSection {
            name: Cow::Borrowed("molt.callable_table"),
            data: Cow::Borrowed(attestation),
        });
    }
    module.finish()
}

fn module_with_two_tables(
    instructions: &[Instruction<'static>],
    active_table: Option<u32>,
    export_function: bool,
    export_table: Option<u32>,
) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import(
        "env",
        "canonical_table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 2,
            maximum: Some(16),
            shared: false,
        }),
    );
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 2,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    if export_function || export_table.is_some() {
        let mut exports = ExportSection::new();
        if export_function {
            exports.export("root", ExportKind::Func, 0);
        }
        if let Some(table_index) = export_table {
            exports.export("observable_table", ExportKind::Table, table_index);
        }
        module.section(&exports);
    }
    if let Some(table_index) = active_table {
        let offset = ConstExpr::i32_const(0);
        let mut elements = ElementSection::new();
        elements.active(
            Some(table_index),
            &offset,
            Elements::Functions(Cow::Owned(vec![0])),
        );
        module.section(&elements);
    }
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    for instruction in instructions {
        body.instruction(instruction);
    }
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    module.finish()
}

fn liveness_fixture(
    root_indirect: bool,
    dead_indirect: bool,
    import_dispatch_helper: bool,
    root_calls_dispatch_helper: bool,
    export_table: bool,
) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    if import_dispatch_helper {
        let mut imports = ImportSection::new();
        imports.import("env", "molt_call_indirect0", EntityType::Function(0));
        module.section(&imports);
    }
    let import_count = u32::from(import_dispatch_helper);
    let mut functions = FunctionSection::new();
    for _ in 0..3 {
        functions.function(0);
    }
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut exports = ExportSection::new();
    exports.export("root", ExportKind::Func, import_count);
    if export_table {
        exports.export("callable_table", ExportKind::Table, 0);
    }
    module.section(&exports);
    let mut elements = ElementSection::new();
    elements.active(
        None,
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned(vec![import_count + 1])),
    );
    module.section(&elements);
    let mut code = CodeSection::new();
    for (function_offset, indirect) in [root_indirect, false, dead_indirect]
        .into_iter()
        .enumerate()
    {
        let mut body = Function::new([]);
        if function_offset == 0 && root_calls_dispatch_helper {
            body.instruction(&Instruction::Call(0));
        }
        if indirect {
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::CallIndirect {
                type_index: 0,
                table_index: 0,
            });
        }
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&code);
    module.finish()
}

#[test]
fn scans_import_adjusted_calls_refs_and_active_elements() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("env", "first", EntityType::Function(0));
    imports.import("env", "second", EntityType::Function(0));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 9,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let offset = ConstExpr::i32_const(7);
    let mut elements = ElementSection::new();
    elements.segment(ElementSegment {
        mode: ElementMode::Active {
            table: None,
            offset: &offset,
        },
        elements: Elements::Functions(Cow::Owned(vec![1, 2])),
    });
    module.section(&elements);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(0));
    body.instruction(&Instruction::RefFunc(1));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan module");

    assert_eq!(facts.function_import_count, 2);
    assert_eq!(facts.defined_function_count, 1);
    assert_eq!(facts.function_references[0].function_index, 2);
    assert_eq!(facts.function_references[0].direct_calls, [0]);
    assert_eq!(facts.function_references[0].ref_funcs, [1]);
    assert!(facts.reachable_function_indices.is_empty());
    assert_eq!(facts.referenced_function_indices, [0, 1, 2]);
    assert!(facts.root_function_indices.is_empty());
    assert_eq!(facts.element_function_indices, [1, 2]);
    assert!(facts.declared_function_indices.is_empty());
    assert_eq!(facts.active_element_segments[0].base, 7);
    assert_eq!(facts.active_element_segments[0].item_count, 2);
    assert_eq!(facts.active_function_elements[0].slot, 7);
}

#[test]
fn projects_main_module_init_callees_without_publishing_the_call_graph() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("molt_init___main__", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut entry = Function::new([]);
    entry.instruction(&Instruction::Call(1));
    entry.instruction(&Instruction::End);
    code.function(&entry);
    let mut callee = Function::new([]);
    callee.instruction(&Instruction::End);
    code.function(&callee);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan main module init");

    assert_eq!(facts.main_module_init_direct_calls, [1]);
    assert_eq!(facts.reachable_function_indices, [0, 1]);
    assert_eq!(facts.referenced_function_indices, [0, 1]);
}

#[test]
fn records_first_mutator_but_scans_every_body() {
    let wasm = module_with_bodies(&[
        vec![
            Instruction::I32Const(0),
            Instruction::RefNull(HeapType::FUNC),
            Instruction::TableSet(0),
        ],
        vec![
            Instruction::I32Const(0),
            Instruction::RefNull(HeapType::FUNC),
            Instruction::I32Const(0),
            Instruction::TableFill(0),
        ],
    ]);

    let facts = scan_wasm_link_facts(&wasm).expect("scan mutation module");

    assert_eq!(facts.code_body_count, 2);
    assert_eq!(facts.table_mutations[0].function_index, 0);
    assert_eq!(facts.table_mutations[0].operation, "table.set");
}

#[test]
fn detects_each_prohibited_table_mutation() {
    let cases = [
        (
            vec![
                Instruction::I32Const(0),
                Instruction::RefNull(HeapType::FUNC),
                Instruction::TableSet(0),
            ],
            "table.set",
        ),
        (
            vec![
                Instruction::I32Const(0),
                Instruction::I32Const(0),
                Instruction::I32Const(0),
                Instruction::TableInit {
                    elem_index: 0,
                    table: 0,
                },
            ],
            "table.init",
        ),
        (
            vec![
                Instruction::I32Const(0),
                Instruction::I32Const(0),
                Instruction::I32Const(0),
                Instruction::TableCopy {
                    src_table: 0,
                    dst_table: 0,
                },
            ],
            "table.copy",
        ),
        (
            vec![
                Instruction::RefNull(HeapType::FUNC),
                Instruction::I32Const(0),
                Instruction::TableGrow(0),
                Instruction::Drop,
            ],
            "table.grow",
        ),
        (
            vec![
                Instruction::I32Const(0),
                Instruction::RefNull(HeapType::FUNC),
                Instruction::I32Const(0),
                Instruction::TableFill(0),
            ],
            "table.fill",
        ),
    ];
    for (instructions, expected) in cases {
        let facts =
            scan_wasm_link_facts(&module_with_bodies(&[instructions])).expect("scan mutation");
        assert_eq!(facts.table_mutations[0].operation, expected);
    }
}

#[test]
fn rejects_malformed_and_truncated_wasm() {
    let mut wasm = module_with_bodies(&[vec![Instruction::Nop]]);
    wasm.pop();
    assert!(scan_wasm_link_facts(&wasm).is_err());
    assert!(scan_wasm_link_facts(b"\0asm\x01\0\0").is_err());
}

#[test]
fn rejects_component_encoding_before_scanning_or_publication() {
    let component = Component::new().finish();
    assert!(
        scan_wasm_link_facts(&component)
            .unwrap_err()
            .contains("core WebAssembly module version 1")
    );
    assert!(
        publish_callable_table_attestation(&component, None)
            .unwrap_err()
            .contains("core WebAssembly module version 1")
    );
}

#[test]
fn malformed_attestation_counts_are_bounded_by_encoded_payload() {
    let cases = [
        // Impossible type count.
        {
            let mut payload = vec![1, 1];
            u32::MAX.encode(&mut payload);
            payload
        },
        // No types and an impossible entry count.
        {
            let mut payload = vec![1, 1, 0];
            u32::MAX.encode(&mut payload);
            payload
        },
    ];
    for payload in cases {
        let mut module = Module::new();
        module.section(&CustomSection {
            name: Cow::Borrowed("molt.callable_table"),
            data: Cow::Owned(payload),
        });
        let error = scan_wasm_link_facts(&module.finish()).unwrap_err();
        assert!(error.contains("encoded payload bound"), "{error}");
    }

    let mut impossible_value_count = vec![1, 1, 1, 0];
    u32::MAX.encode(&mut impossible_value_count);
    let error =
        scan_wasm_link_facts(&module_with_callable_table(&[impossible_value_count])).unwrap_err();
    assert!(error.contains("encoded payload bound"), "{error}");
}

#[test]
fn typed_function_reference_dispatch_tracks_table_provenance_and_fails_closed() {
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
        minimum: 1,
        maximum: None,
        shared: false,
    });
    let typed_ref = RefType {
        nullable: true,
        heap_type: HeapType::Concrete(0),
    };
    tables.table(TableType {
        element_type: typed_ref,
        table64: false,
        minimum: 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut exports = ExportSection::new();
    exports.export("root", ExportKind::Func, 0);
    module.section(&exports);
    let offset = ConstExpr::i32_const(0);
    let expressions = [ConstExpr::ref_func(0)];
    let mut elements = ElementSection::new();
    elements.active(
        Some(1),
        &offset,
        Elements::Expressions(typed_ref, Cow::Borrowed(&expressions)),
    );
    module.section(&elements);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::TableGet(1));
    body.instruction(&Instruction::RefAsNonNull);
    body.instruction(&Instruction::CallRef(0));
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    let wasm = module.finish();

    let facts = scan_wasm_link_facts(&wasm).expect("scan typed-reference dispatch");
    assert_eq!(facts.function_reference_dispatch_functions, [0]);
    assert!(facts.reachable_function_reference_dispatch);
    assert_eq!(
        facts.reachable_table_reads,
        [WasmTableRead {
            function_index: 0,
            table_index: 1,
        }]
    );
    assert!(
        publish_callable_table_attestation(&wasm, None)
            .unwrap_err()
            .contains("function-reference dispatch can escape canonical table 0")
    );
}

#[test]
fn return_call_ref_is_part_of_the_same_dispatch_authority() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("root", ExportKind::Func, 0);
    module.section(&exports);
    let mut elements = ElementSection::new();
    elements.declared(Elements::Functions(Cow::Owned(vec![0])));
    module.section(&elements);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::RefFunc(0));
    body.instruction(&Instruction::ReturnCallRef(0));
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan return_call_ref");
    assert_eq!(facts.function_reference_dispatch_functions, [0]);
    assert!(facts.reachable_function_reference_dispatch);
}

#[test]
fn skips_huge_custom_and_data_payloads_without_fact_allocation() {
    let payload = vec![0xA5; 4 * 1024 * 1024];
    let mut module = Module::new();
    module.section(&CustomSection {
        name: Cow::Borrowed("huge"),
        data: Cow::Borrowed(&payload),
    });
    let mut data = DataSection::new();
    data.passive(payload.iter().copied());
    module.section(&data);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan payload module");

    assert_eq!(facts.operator_count, 0);
    assert!(facts.function_references.is_empty());
}

#[test]
fn mutation_free_full_scan_reports_every_operator() {
    let wasm = module_with_bodies(&[
        vec![Instruction::Nop, Instruction::Call(1)],
        vec![Instruction::RefFunc(0), Instruction::Drop],
    ]);

    let facts = scan_wasm_link_facts(&wasm).expect("scan clean module");

    assert!(facts.table_mutations.is_empty());
    assert_eq!(facts.code_body_count, 2);
    assert!(facts.operator_count >= 6);
}

#[test]
fn validator_rejects_duplicate_sections_and_invalid_indices() {
    let mut duplicate = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    duplicate.section(&types);
    duplicate.section(&types);
    assert!(scan_wasm_link_facts(&duplicate.finish()).is_err());

    let bad_call = module_with_bodies(&[vec![Instruction::Call(99)]]);
    assert!(scan_wasm_link_facts(&bad_call).is_err());

    let bad_ref = module_with_bodies(&[vec![Instruction::RefFunc(99), Instruction::Drop]]);
    assert!(scan_wasm_link_facts(&bad_ref).is_err());

    let bad_table = module_with_bodies(&[vec![
        Instruction::I32Const(0),
        Instruction::RefNull(HeapType::FUNC),
        Instruction::TableSet(1),
    ]]);
    assert!(scan_wasm_link_facts(&bad_table).is_err());
}

#[test]
fn validates_matching_callable_table_attestation() {
    let wasm = module_with_callable_table(&[callable_table_attestation(7, 0, 0)]);

    let facts = scan_wasm_link_facts(&wasm).expect("validate attested module");

    assert!(facts.callable_table_attestation_present);
    assert_eq!(
        facts.callable_table_entries,
        [WasmCallableTableEntryFact {
            slot: 7,
            function_index: 0,
            type_index: 0,
            role: 0,
        }]
    );
}

#[test]
fn rejects_stale_or_duplicate_callable_table_attestations() {
    let stale = module_with_callable_table(&[callable_table_attestation(7, 1, 0)]);
    assert!(
        scan_wasm_link_facts(&stale)
            .unwrap_err()
            .contains("disagrees with final module facts")
    );

    let attestation = callable_table_attestation(7, 0, 0);
    let duplicate = module_with_callable_table(&[attestation.clone(), attestation]);
    assert!(
        scan_wasm_link_facts(&duplicate)
            .unwrap_err()
            .contains("duplicate molt.callable_table")
    );

    let mut layout_payload = Vec::new();
    for value in [1u32, 0, 0, 0, 0] {
        value.encode(&mut layout_payload);
    }
    let mut duplicate_layout = Module::new();
    for _ in 0..2 {
        duplicate_layout.section(&CustomSection {
            name: Cow::Borrowed("molt.callable_table.layout"),
            data: Cow::Borrowed(&layout_payload),
        });
    }
    assert!(
        scan_wasm_link_facts(&duplicate_layout.finish())
            .unwrap_err()
            .contains("duplicate molt.callable_table.layout")
    );
}

#[test]
fn active_element_publication_is_last_wins_and_ref_null_clears() {
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
        minimum: 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let offset = ConstExpr::i32_const(0);
    let mut elements = ElementSection::new();
    elements.active(None, &offset, Elements::Functions(Cow::Owned(vec![0])));
    elements.active(
        None,
        &offset,
        Elements::Expressions(
            RefType::FUNCREF,
            Cow::Owned(vec![ConstExpr::ref_null(HeapType::FUNC)]),
        ),
    );
    module.section(&elements);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan overlapping elements");

    assert_eq!(facts.active_element_segments.len(), 2);
    assert!(facts.active_function_elements.is_empty());
    assert!(facts.callable_table_entries.is_empty());
}

#[test]
fn extracts_function_type_from_recursive_gc_group_with_typed_reference() {
    let array = SubType {
        is_final: true,
        supertype_idx: None,
        composite_type: EncoderCompositeType {
            inner: EncoderCompositeInnerType::Array(ArrayType(FieldType {
                element_type: StorageType::I8,
                mutable: false,
            })),
            shared: false,
            descriptor: None,
            describes: None,
        },
    };
    let function_type = SubType {
        is_final: true,
        supertype_idx: None,
        composite_type: EncoderCompositeType {
            inner: EncoderCompositeInnerType::Func(FuncType::new(
                [ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(0),
                })],
                [],
            )),
            shared: false,
            descriptor: None,
            describes: None,
        },
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().rec(vec![array, function_type]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(1);
    module.section(&functions);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan modern type group");

    assert_eq!(facts.function_types.len(), 2);
    assert!(facts.function_types[0].is_none());
    assert_eq!(facts.function_types[1].as_ref().unwrap().type_index, 1);
    assert!(!facts.function_types[1].as_ref().unwrap().params[0].is_empty());
    assert_eq!(facts.function_type_indices, [1]);
}

#[test]
fn exported_and_global_ref_functions_share_the_root_authority() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    functions.function(0);
    module.section(&functions);
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::FUNCREF,
            mutable: false,
            shared: false,
        },
        &ConstExpr::ref_func(1),
    );
    module.section(&globals);
    let mut exports = ExportSection::new();
    exports.export("entry", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    for _ in 0..2 {
        let mut body = Function::new([]);
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan root declarations");

    assert_eq!(facts.root_function_indices, [0, 1]);
}

#[test]
fn table_mutations_name_target_and_copy_source_tables() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut tables = TableSection::new();
    for _ in 0..2 {
        tables.table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 1,
            maximum: None,
            shared: false,
        });
    }
    module.section(&tables);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::RefNull(HeapType::FUNC));
    body.instruction(&Instruction::TableSet(1));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::TableCopy {
        src_table: 1,
        dst_table: 0,
    });
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan table mutations");

    assert_eq!(facts.table_mutations.len(), 2);
    assert_eq!(facts.table_mutations[0].operation, "table.copy");
    assert_eq!(facts.table_mutations[0].table_index, 0);
    assert_eq!(facts.table_mutations[0].source_table_index, Some(1));
    assert_eq!(facts.table_mutations[1].operation, "table.set");
    assert_eq!(facts.table_mutations[1].table_index, 1);
    assert_eq!(facts.table_mutations[1].source_table_index, None);
}

#[test]
fn classifies_multi_table_topology_and_rejects_callable_escape() {
    let active_escape = module_with_two_tables(&[], Some(1), false, Some(1));
    let facts = scan_wasm_link_facts(&active_escape).expect("scan active table escape");
    assert_eq!(facts.tables.len(), 2);
    assert!(facts.tables[0].imported);
    assert!(!facts.tables[1].imported);
    assert!(facts.tables.iter().all(|table| table.untyped_funcref));
    assert_eq!(facts.tables[0].minimum, 2);
    assert_eq!(facts.tables[0].maximum, Some(16));
    assert_eq!(facts.active_function_elements[0].table_index, 1);
    assert!(
        publish_callable_table_attestation(&active_escape, None)
            .unwrap_err()
            .contains("escapes canonical table 0")
    );

    let indirect_escape = module_with_two_tables(
        &[
            Instruction::I32Const(0),
            Instruction::CallIndirect {
                type_index: 0,
                table_index: 1,
            },
        ],
        None,
        true,
        None,
    );
    let facts = scan_wasm_link_facts(&indirect_escape).expect("scan indirect table escape");
    assert_eq!(facts.indirect_call_tables, [1]);
    assert!(
        publish_callable_table_attestation(&indirect_escape, None)
            .unwrap_err()
            .contains("indirect callable dispatch escapes")
    );
    let dead_indirect_escape = module_with_two_tables(
        &[
            Instruction::I32Const(0),
            Instruction::CallIndirect {
                type_index: 0,
                table_index: 1,
            },
        ],
        None,
        false,
        None,
    );
    let dead_facts =
        scan_wasm_link_facts(&dead_indirect_escape).expect("scan dead indirect escape");
    assert!(dead_facts.reachable_indirect_call_tables.is_empty());
    publish_callable_table_attestation(&dead_indirect_escape, None)
        .expect("dead nonzero indirect body does not reject publication");

    let copy_escape = module_with_two_tables(
        &[
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::TableCopy {
                src_table: 0,
                dst_table: 1,
            },
        ],
        None,
        true,
        None,
    );
    let facts = scan_wasm_link_facts(&copy_escape).expect("scan table copy escape");
    assert_eq!(facts.table_mutations[0].source_table_index, Some(0));
    assert!(
        publish_callable_table_attestation(&copy_escape, None)
            .unwrap_err()
            .contains("mutates or escapes callable table 0")
    );

    let dead_copy = module_with_two_tables(
        &[
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::TableCopy {
                src_table: 0,
                dst_table: 1,
            },
        ],
        None,
        false,
        None,
    );
    let dead_facts = scan_wasm_link_facts(&dead_copy).expect("scan dead table copy");
    assert!(dead_facts.reachable_table_mutations.is_empty());
    publish_callable_table_attestation(&dead_copy, None)
        .expect("dead table-copy body does not reject publication");
}

#[test]
fn separates_roots_elements_and_reachable_dynamic_dispatch() {
    let dead_indirect = scan_wasm_link_facts(&liveness_fixture(false, true, false, false, false))
        .expect("scan dead indirect body");
    assert_eq!(dead_indirect.root_function_indices, [0]);
    assert_eq!(dead_indirect.element_function_indices, [1]);
    assert_eq!(dead_indirect.dynamic_dispatch_functions, [2]);
    assert!(dead_indirect.dynamic_table_dispatch);
    assert!(!dead_indirect.reachable_dynamic_dispatch);

    let reachable_indirect =
        scan_wasm_link_facts(&liveness_fixture(true, false, false, false, false))
            .expect("scan reachable indirect body");
    assert_eq!(reachable_indirect.dynamic_dispatch_functions, [0]);
    assert!(reachable_indirect.reachable_dynamic_dispatch);

    let unused_import = scan_wasm_link_facts(&liveness_fixture(false, false, true, false, false))
        .expect("scan unused dispatch import");
    assert!(unused_import.dynamic_table_dispatch);
    assert!(unused_import.dynamic_dispatch_functions.is_empty());
    assert!(!unused_import.reachable_dynamic_dispatch);

    let reachable_import = scan_wasm_link_facts(&liveness_fixture(false, false, true, true, false))
        .expect("scan reachable dispatch import");
    assert_eq!(reachable_import.dynamic_dispatch_functions, [1]);
    assert!(reachable_import.reachable_dynamic_dispatch);

    let exported_table = scan_wasm_link_facts(&liveness_fixture(false, false, false, false, true))
        .expect("scan exported table");
    assert_eq!(exported_table.exported_table_indices, [0]);

    let mut exported_import = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    exported_import.section(&types);
    let mut imports = ImportSection::new();
    imports.import("env", "molt_call_indirect0", EntityType::Function(0));
    exported_import.section(&imports);
    let mut exports = ExportSection::new();
    exports.export("dispatch", ExportKind::Func, 0);
    exported_import.section(&exports);
    let exported_import =
        scan_wasm_link_facts(&exported_import.finish()).expect("scan exported dispatch import");
    assert_eq!(exported_import.root_function_indices, [0]);
    assert!(exported_import.reachable_dynamic_dispatch);
}

#[test]
fn reachable_dispatch_and_ref_func_follow_call_chain_cycles_only_from_roots() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    for _ in 0..4 {
        functions.function(0);
    }
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 1,
        maximum: None,
        shared: false,
    });
    module.section(&tables);
    let mut exports = ExportSection::new();
    exports.export("root", ExportKind::Func, 0);
    module.section(&exports);
    let mut elements = ElementSection::new();
    elements.declared(Elements::Functions(Cow::Owned(vec![1, 2])));
    module.section(&elements);
    let mut code = CodeSection::new();
    let bodies = [
        vec![Instruction::Call(1)],
        vec![Instruction::Call(2), Instruction::Call(0)],
        vec![
            Instruction::I32Const(0),
            Instruction::CallIndirect {
                type_index: 0,
                table_index: 0,
            },
            Instruction::RefFunc(1),
            Instruction::Drop,
        ],
        vec![Instruction::RefFunc(2), Instruction::Drop],
    ];
    for instructions in bodies {
        let mut body = Function::new([]);
        for instruction in instructions {
            body.instruction(&instruction);
        }
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan cyclic reachability");

    assert_eq!(facts.root_function_indices, [0]);
    assert_eq!(facts.dynamic_dispatch_functions, [2]);
    assert!(facts.reachable_dynamic_dispatch);
    assert_eq!(facts.function_references.last().unwrap().function_index, 3);
    assert_eq!(facts.function_references.last().unwrap().ref_funcs, [2]);
}

#[test]
fn passive_and_declared_membership_is_not_a_root_without_table_init() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    for _ in 0..3 {
        functions.function(0);
    }
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("root", ExportKind::Func, 0);
    module.section(&exports);
    let mut elements = ElementSection::new();
    elements.passive(Elements::Functions(Cow::Owned(vec![1])));
    elements.declared(Elements::Functions(Cow::Owned(vec![2])));
    module.section(&elements);
    let mut code = CodeSection::new();
    for _ in 0..3 {
        let mut body = Function::new([]);
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    module.section(&code);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan declared membership");

    assert_eq!(facts.root_function_indices, [0]);
    assert_eq!(facts.element_function_indices, [1, 2]);
    assert_eq!(facts.declared_function_indices, [1, 2]);
    assert!(facts.active_function_elements.is_empty());
    assert!(!facts.reachable_dynamic_dispatch);
}
