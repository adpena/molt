use std::borrow::Cow;

use wasm_encoder::Encode;
use wasm_encoder::{
    ArrayType, CodeSection, Component, CompositeInnerType as EncoderCompositeInnerType,
    CompositeType as EncoderCompositeType, ConstExpr, CustomSection, DataSection, ElementMode,
    ElementSection, ElementSegment, Elements, EntityType, ExportKind, ExportSection, FieldType,
    FuncType, Function, FunctionSection, GlobalSection, GlobalType, HeapType, ImportSection,
    Instruction, MemorySection, MemoryType, Module, RefType, StorageType, SubType, TableSection,
    TableType, TagKind, TagType, TypeSection, ValType,
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

#[test]
fn section_publication_preserves_empty_and_deferred_code_bytes() {
    for bodies in [
        vec![],
        vec![vec![Instruction::Nop]],
        vec![
            vec![Instruction::Nop],
            vec![Instruction::I32Const(23), Instruction::Drop],
        ],
    ] {
        let original = module_with_bodies(&bodies);
        let mut published = Module::new();
        scan::scan_wasm_link_facts_with_sections(
            &original,
            Some(&mut |id, data| {
                published.section(&wasm_encoder::RawSection { id, data });
                Ok(())
            }),
        )
        .expect("valid encoded module passes section publication");
        assert_eq!(published.finish(), original);
    }
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

fn linking_global_symbol_payload(symbols: &[(&str, u32)]) -> Vec<u8> {
    let mut entries = Vec::new();
    for (name, global_index) in symbols {
        entries.push(2); // WASM_SYMBOL_TYPE_GLOBAL
        0u32.encode(&mut entries); // defined global binding
        global_index.encode(&mut entries);
        u32::try_from(name.len())
            .expect("fixture symbol length fits u32")
            .encode(&mut entries);
        entries.extend_from_slice(name.as_bytes());
    }
    let mut symbol_table = Vec::new();
    u32::try_from(symbols.len())
        .expect("fixture symbol count fits u32")
        .encode(&mut symbol_table);
    symbol_table.extend(entries);

    let mut linking = Vec::new();
    2u32.encode(&mut linking);
    linking.push(8); // WASM_SYMBOL_TABLE
    u32::try_from(symbol_table.len())
        .expect("fixture symbol table length fits u32")
        .encode(&mut linking);
    linking.extend(symbol_table);
    linking
}

fn linking_data_symbol_payload(name: &str, size: u32, flags: wasmparser::SymbolFlags) -> Vec<u8> {
    let mut symbol_table = Vec::new();
    1u32.encode(&mut symbol_table);
    symbol_table.push(1); // data symbol; parsed by wasmparser's Linking reader
    flags.bits().encode(&mut symbol_table);
    u32::try_from(name.len())
        .expect("fixture symbol length fits u32")
        .encode(&mut symbol_table);
    symbol_table.extend_from_slice(name.as_bytes());
    if !flags.contains(wasmparser::SymbolFlags::UNDEFINED) {
        0u32.encode(&mut symbol_table); // segment index
        0u32.encode(&mut symbol_table); // segment offset
        size.encode(&mut symbol_table);
    }

    let mut linking = Vec::new();
    2u32.encode(&mut linking);
    linking.push(8); // symbol-table subsection; parsed by wasmparser
    u32::try_from(symbol_table.len())
        .expect("fixture symbol table length fits u32")
        .encode(&mut linking);
    linking.extend(symbol_table);
    linking
}

#[test]
fn linking_direct_global_symbol_attests_split_runtime_got_without_debug_names() {
    let mut module = Module::new();
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(i32::MIN),
    );
    module.section(&globals);
    let linking = linking_global_symbol_payload(&[("GOT.data.internal.molt_PyType_Type", 0)]);
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking),
    });

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan GOT fact module");

    assert!(facts.linking_symbol_table_present);
    assert_eq!(
        facts.split_runtime_got_data_globals,
        vec![WasmGotDataGlobalFact {
            symbol: "molt_PyType_Type".to_string(),
            global_index: 0,
            initial_address: Some(0x8000_0000),
            flags: 0,
            defined: true,
        }]
    );
}

fn relocated_data_symbol_module(flags: wasmparser::SymbolFlags) -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(i32::MIN),
    );
    module.section(&globals);
    let mut exports = ExportSection::new();
    exports.export("get_type_address", ExportKind::Func, 0);
    module.section(&exports);
    let code = [
        1,    // body count
        8,    // body size
        0,    // local declaration count
        0x23, // global.get
        0x80, 0x80, 0x80, 0x80, 0x00, // relocatable five-byte global index 0
        0x0b, // end
    ];
    module.section(&wasm_encoder::RawSection {
        id: 10,
        data: &code,
    });
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking_data_symbol_payload("molt_PyType_Type", 208, flags)),
    });
    let mut relocation = Vec::new();
    4u32.encode(&mut relocation); // code section ordinal
    1u32.encode(&mut relocation); // relocation count
    relocation.push(7); // R_WASM_GLOBAL_INDEX_LEB
    4u32.encode(&mut relocation); // code-payload offset of global index
    0u32.encode(&mut relocation); // linking symbol-table index
    module.section(&CustomSection {
        name: Cow::Borrowed("reloc.CODE"),
        data: Cow::Owned(relocation),
    });
    module.finish()
}

#[test]
fn linking_data_symbol_and_code_relocation_attest_real_wasm_ld_got_shape() {
    let module = relocated_data_symbol_module(wasmparser::SymbolFlags::EXPLICIT_NAME);

    let facts = scan_wasm_link_facts(&module).expect("scan real linker GOT shape");

    assert!(facts.linking_symbol_table_present);
    assert_eq!(
        facts.split_runtime_got_data_globals,
        vec![WasmGotDataGlobalFact {
            symbol: "molt_PyType_Type".to_string(),
            global_index: 0,
            initial_address: Some(0x8000_0000),
            flags: wasmparser::SymbolFlags::EXPLICIT_NAME.bits(),
            defined: true,
        }]
    );
}

#[test]
fn linking_data_symbol_got_relocation_records_binding_for_consumer_policy() {
    for flags in [
        wasmparser::SymbolFlags::EXPLICIT_NAME | wasmparser::SymbolFlags::BINDING_WEAK,
        wasmparser::SymbolFlags::EXPLICIT_NAME | wasmparser::SymbolFlags::BINDING_LOCAL,
        wasmparser::SymbolFlags::EXPLICIT_NAME | wasmparser::SymbolFlags::UNDEFINED,
    ] {
        let facts = scan_wasm_link_facts(&relocated_data_symbol_module(flags))
            .expect("record GOT binding evidence");
        let got = &facts.split_runtime_got_data_globals[0];
        assert_eq!(got.flags, flags.bits());
        assert_eq!(
            got.defined,
            !flags.contains(wasmparser::SymbolFlags::UNDEFINED)
        );
        assert_eq!(got.initial_address, Some(0x8000_0000));
    }
}

#[test]
fn linking_symbol_table_rejects_duplicate_split_runtime_got_authority() {
    let mut module = Module::new();
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(1),
    );
    module.section(&globals);
    let linking = linking_global_symbol_payload(&[
        ("GOT.data.internal.molt_PyType_Type", 0),
        ("GOT.data.internal.molt_PyType_Type", 0),
    ]);
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking),
    });

    let error = scan_wasm_link_facts(&module.finish()).unwrap_err();
    assert!(error.contains("duplicate split-runtime GOT data symbol"));
}

#[test]
fn linking_symbol_table_rejects_duplicate_split_runtime_got_global_index() {
    let mut module = Module::new();
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(1),
    );
    module.section(&globals);
    let linking = linking_global_symbol_payload(&[
        ("GOT.data.internal.molt_PyType_Type", 0),
        ("GOT.data.internal.molt_PyList_Type", 0),
    ]);
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking),
    });

    let error = scan_wasm_link_facts(&module.finish()).unwrap_err();
    assert!(error.contains("duplicate split-runtime GOT data global index"));
}

#[test]
fn linking_symbol_table_records_noncanonical_got_global_shapes() {
    for (global_type, initializer, initial_address) in [
        (
            GlobalType {
                val_type: ValType::I64,
                mutable: false,
                shared: false,
            },
            ConstExpr::i64_const(1),
            None,
        ),
        (
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            ConstExpr::i32_const(1),
            Some(1),
        ),
    ] {
        let mut module = Module::new();
        let mut globals = GlobalSection::new();
        globals.global(global_type, &initializer);
        module.section(&globals);
        let linking = linking_global_symbol_payload(&[("GOT.data.internal.molt_PyType_Type", 0)]);
        module.section(&CustomSection {
            name: Cow::Borrowed("linking"),
            data: Cow::Owned(linking),
        });

        let facts = scan_wasm_link_facts(&module.finish()).expect("scan GOT shape evidence");
        assert_eq!(
            facts.split_runtime_got_data_globals[0].initial_address,
            initial_address
        );
    }
}

#[test]
fn linking_symbol_table_records_imported_got_global_target() {
    let mut module = Module::new();
    let mut imports = ImportSection::new();
    imports.import(
        "env",
        "placeholder",
        EntityType::Global(GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        }),
    );
    module.section(&imports);
    let linking = linking_global_symbol_payload(&[("GOT.data.internal.molt_PyType_Type", 0)]);
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking),
    });

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan imported GOT target");
    assert_eq!(
        facts.split_runtime_got_data_globals[0].initial_address,
        None
    );
}

#[test]
fn linking_section_without_symbol_table_is_attested_as_absent() {
    let mut module = Module::new();
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(vec![2]),
    });

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan linking-only module");
    assert!(!facts.linking_symbol_table_present);
    assert!(facts.split_runtime_got_data_globals.is_empty());
}

#[test]
fn canonical_extern_type_facts_cover_function_global_memory_table_and_tag_edges() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::I64]);
    types.ty().function([ValType::I32], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("molt_runtime", "call", EntityType::Function(0));
    imports.import(
        "env",
        "global",
        EntityType::Global(GlobalType {
            val_type: ValType::I32,
            mutable: false,
            shared: false,
        }),
    );
    imports.import(
        "env",
        "memory",
        EntityType::Memory(wasm_encoder::MemoryType {
            minimum: 2,
            maximum: Some(4),
            memory64: false,
            shared: true,
            page_size_log2: None,
        }),
    );
    imports.import(
        "env",
        "table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 3,
            maximum: Some(8),
            shared: false,
        }),
    );
    imports.import(
        "env",
        "tag",
        EntityType::Tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: 1,
        }),
    );
    module.section(&imports);
    let mut exports = ExportSection::new();
    exports.export("call", ExportKind::Func, 0);
    exports.export("global", ExportKind::Global, 0);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("table", ExportKind::Table, 0);
    exports.export("tag", ExportKind::Tag, 0);
    module.section(&exports);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan extern type module");

    assert_eq!(facts.canonical_import_types.len(), 5);
    assert_eq!(facts.canonical_export_types.len(), 5);
    let call_import = facts
        .canonical_import_types
        .iter()
        .find(|fact| fact.name == "call")
        .expect("call import fact");
    assert_eq!(call_import.module, "molt_runtime");
    assert_eq!((call_import.kind, call_import.index), (0, 0));
    assert_eq!(
        call_import.extern_type,
        WasmCanonicalExternType::Function {
            exact: false,
            params: vec![vec![0x7f]],
            results: vec![vec![0x7e]],
        }
    );
    let type_by_name = |name: &str| {
        &facts
            .canonical_import_types
            .iter()
            .find(|fact| fact.name == name)
            .unwrap_or_else(|| panic!("missing {name} import fact"))
            .extern_type
    };
    assert_eq!(
        type_by_name("global"),
        &WasmCanonicalExternType::Global {
            value_type: vec![0x7f],
            mutable: false,
            shared: false,
        }
    );
    assert_eq!(
        type_by_name("memory"),
        &WasmCanonicalExternType::Memory {
            memory64: false,
            shared: true,
            minimum: 2,
            maximum: Some(4),
            page_size_log2: None,
        }
    );
    assert_eq!(
        facts
            .canonical_import_types
            .iter()
            .find(|fact| fact.name == "memory")
            .map(|fact| (fact.kind, fact.index)),
        Some((2, 0))
    );
    assert_eq!(
        facts
            .canonical_export_types
            .iter()
            .find(|fact| fact.name == "table")
            .map(|fact| (fact.kind, fact.index)),
        Some((1, 0))
    );
    assert_eq!(facts.defined_memory_count, 0);
    assert_eq!(
        type_by_name("table"),
        &WasmCanonicalExternType::Table {
            table64: false,
            shared: false,
            minimum: 3,
            maximum: Some(8),
            element_type: vec![0x70],
        }
    );
    assert_eq!(
        type_by_name("tag"),
        &WasmCanonicalExternType::Tag {
            tag_kind: "exception".to_string(),
            params: vec![vec![0x7f]],
            results: vec![],
        }
    );
    assert!(
        facts
            .canonical_import_types
            .windows(2)
            .all(|pair| pair[0] <= pair[1])
    );
    assert!(
        facts
            .canonical_export_types
            .windows(2)
            .all(|pair| pair[0] <= pair[1])
    );
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
    assert_eq!(facts.custom_section_names, ["huge"]);
}

#[test]
fn projects_defined_memory_count_without_python_section_reparse() {
    let mut module = Module::new();
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: Some(2),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    module.section(&exports);

    let facts = scan_wasm_link_facts(&module.finish()).expect("scan defined memory");

    assert_eq!(facts.defined_memory_count, 1);
    let memory = facts
        .canonical_export_types
        .iter()
        .find(|fact| fact.name == "memory")
        .expect("memory export fact");
    assert_eq!((memory.kind, memory.index), (2, 0));
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
        supertype_idxs: Vec::new(),
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
        supertype_idxs: Vec::new(),
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

#[test]
fn linking_facts_preserve_symbol_ordinals_and_resolve_implicit_import_names() {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("env", "imported", EntityType::Function(0));
    module.section(&imports);
    // First row is a section symbol; it must not renumber the function symbol.
    let table = vec![2, 3, 0, 0, 0, 0x10, 0];
    let mut linking = vec![2, 8];
    (table.len() as u32).encode(&mut linking);
    linking.extend(table);
    module.section(&CustomSection {
        name: Cow::Borrowed("linking"),
        data: Cow::Owned(linking),
    });
    let mut names = wasm_encoder::NameSection::new();
    let mut functions = wasm_encoder::NameMap::new();
    functions.append(0, "debug_imported");
    names.functions(&functions);
    module.section(&names);
    let facts = scan_wasm_link_facts(&module.finish()).expect("scan symbol projection");
    assert_eq!(facts.linking_symbols.len(), 1);
    assert_eq!(facts.linking_symbols[0].symbol_index, 1);
    assert_eq!(facts.linking_symbols[0].name, "imported");
    assert_eq!(facts.linking_symbols[0].kind, "function");
    assert_eq!(
        facts.function_names,
        vec![(0, "debug_imported".to_string())]
    );
}
